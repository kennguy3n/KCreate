// Bundle entry point. The host's extension runtime imports
// `mount` / `unmount` from this file to attach the panel into the
// declared view slot.
import { createRoot, type Root } from "react-dom/client";
import { Panel } from "./panel";

interface MountContext {
  rootElement: HTMLElement;
  activeCommunityId?: string;
  activeConversationId?: string;
}

const roots = new WeakMap<HTMLElement, Root>();

export function mount(ctx: MountContext): void {
  const existing = roots.get(ctx.rootElement);
  if (existing) {
    existing.unmount();
  }
  const root = createRoot(ctx.rootElement);
  // `exactOptionalPropertyTypes` rejects passing `string | undefined`
  // into an optional `string` prop verbatim, so build the props
  // object dynamically and only set each key when the host supplied
  // a value. This keeps the `Panel`'s prop contract honest
  // (undefined != absent) without weakening it.
  const props: { activeCommunityId?: string; activeConversationId?: string } = {};
  if (ctx.activeCommunityId !== undefined) {
    props.activeCommunityId = ctx.activeCommunityId;
  }
  if (ctx.activeConversationId !== undefined) {
    props.activeConversationId = ctx.activeConversationId;
  }
  root.render(<Panel {...props} />);
  roots.set(ctx.rootElement, root);
}

export function unmount(rootElement: HTMLElement): void {
  const root = roots.get(rootElement);
  if (!root) {
    return;
  }
  root.unmount();
  roots.delete(rootElement);
}
