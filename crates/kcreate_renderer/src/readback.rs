//! GPU → CPU pixel transfer.
//!
//! The wgpu mapping API requires the destination buffer to be padded
//! to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] per row. This module
//! handles the row-padding round-trip and unpads the bytes into a
//! tightly-packed RGBA8 buffer that downstream consumers can use
//! directly (no row-stride math needed in Electron).
//!
//! The caller passes in the staging buffer (owned by
//! [`crate::surface::OffscreenSurface`]) so allocations are amortized
//! across frames — the buffer is recreated only on resize.

use crate::gpu::bytes_per_row_aligned;
use crate::{RendererError, Result};

/// Read `texture` (assumed `Rgba8Unorm`) into a tightly-packed RGBA8
/// `Vec<u8>` of length `width * height * 4`. `staging` must have been
/// sized for `bytes_per_row_aligned(width) * height` bytes.
pub fn read_texture_to_vec(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    staging: &wgpu::Buffer,
    width: u32,
    height: u32,
    out: &mut Vec<u8>,
) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    let padded_bytes_per_row = bytes_per_row_aligned(width);
    let required = u64::from(padded_bytes_per_row) * u64::from(height);
    if staging.size() < required {
        return Err(RendererError::Wgpu(format!(
            "readback staging buffer is {} bytes but {required} are required for {width}x{height}",
            staging.size()
        )));
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("kcreate-readback-encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    let slice = staging.slice(..required);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });

    // Drain the device until the map completes. `device.poll` blocks
    // (in `wait` mode) until all submitted work + map callbacks fire.
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let map_result = rx
        .recv()
        .map_err(|e| RendererError::Wgpu(format!("readback channel closed: {e}")))?
        .map_err(|e| RendererError::Wgpu(format!("buffer map failed: {e}")));

    // Always unmap before returning, even on map failure, to leave the
    // buffer in a reusable state for the next frame.
    let result = match map_result {
        Ok(()) => {
            let view = slice.get_mapped_range();
            let row_bytes = (width * 4) as usize;
            out.clear();
            out.reserve(row_bytes * height as usize);
            if padded_bytes_per_row as usize == row_bytes {
                out.extend_from_slice(&view[..row_bytes * height as usize]);
            } else {
                for row in 0..height as usize {
                    let start = row * padded_bytes_per_row as usize;
                    out.extend_from_slice(&view[start..start + row_bytes]);
                }
            }
            drop(view);
            Ok(())
        }
        Err(e) => Err(e),
    };
    staging.unmap();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(async {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok()
        })?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("test"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some((device, queue))
    }

    #[test]
    fn readback_unpads_rows() {
        let Some((device, queue)) = test_device() else {
            return;
        };
        // width=7 forces row-padding (28 bytes/row, alignment 256 -> padded 256).
        let w = 7;
        let h = 3;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..(w * h) {
            pixels.extend_from_slice(&[(i as u8), (i as u8 + 1), (i as u8 + 2), 255]);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::empty());
        let mut out = Vec::new();
        let padded = bytes_per_row_aligned(w);
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        read_texture_to_vec(&device, &queue, &texture, &staging, w, h, &mut out).unwrap();
        assert_eq!(out.len(), (w * h * 4) as usize);
        assert_eq!(out, pixels);
    }
}
