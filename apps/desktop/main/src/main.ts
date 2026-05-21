// Electron main process entry. Owns the BrowserWindow and the
// renderer-side IPC handlers that proxy to the Rust kcreate_bridge
// native addon.

import { app, BrowserWindow, ipcMain } from "electron";
import * as path from "node:path";

import { loadBridge, type Bridge } from "./bridge";

let bridge: Bridge | null = null;

function getBridge(): Bridge {
  if (!bridge) {
    bridge = loadBridge();
  }
  return bridge;
}

function createWindow(): BrowserWindow {
  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    backgroundColor: "#1e1e1e",
    show: false,
    webPreferences: {
      preload: path.join(__dirname, "..", "..", "preload", "dist", "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  const devUrl = process.env["KCREATE_DEV_RENDERER_URL"];
  if (devUrl) {
    void win.loadURL(devUrl);
  } else {
    void win.loadFile(
      path.join(__dirname, "..", "..", "renderer", "dist", "index.html"),
    );
  }

  win.once("ready-to-show", () => win.show());
  return win;
}

function registerIpcHandlers(): void {
  ipcMain.handle("kcreate/renderer/init", (_e, width: number, height: number) =>
    getBridge().rendererInit(width, height),
  );
  ipcMain.handle("kcreate/renderer/shutdown", () => {
    getBridge().rendererShutdown();
  });
  ipcMain.handle(
    "kcreate/renderer/resize",
    (_e, width: number, height: number) => getBridge().rendererResize(width, height),
  );
  ipcMain.handle(
    "kcreate/renderer/setViewport",
    (_e, panX: number, panY: number, zoom: number) =>
      getBridge().rendererSetViewport(panX, panY, zoom),
  );
  ipcMain.handle(
    "kcreate/renderer/invalidate",
    (
      _e,
      region: { x: number; y: number; width: number; height: number } | null,
    ) =>
      getBridge().rendererInvalidate(
        region?.x ?? null,
        region?.y ?? null,
        region?.width ?? null,
        region?.height ?? null,
      ),
  );
  ipcMain.handle("kcreate/renderer/render", (_e, sceneJson: string) =>
    getBridge().rendererRender(sceneJson),
  );
  ipcMain.handle("kcreate/renderer/getFrame", () =>
    getBridge().rendererGetFrame(),
  );
  ipcMain.handle("kcreate/renderer/frameInfo", () =>
    getBridge().rendererFrameInfo(),
  );
}

void app.whenReady().then(() => {
  registerIpcHandlers();
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

app.on("will-quit", () => {
  if (bridge) {
    bridge.rendererShutdown();
    bridge = null;
  }
});
