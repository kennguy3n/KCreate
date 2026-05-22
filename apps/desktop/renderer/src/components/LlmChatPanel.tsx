// LlmChatPanel — local chat assistant UI for the AI Assist tab.
//
// Talks to `window.kcreate.llm.*`, which forwards to the
// `llama-server` sidecar over loopback. No streaming in Phase 1;
// each turn is a full request/response. Pre-built quick actions
// inject system-context prompts so the model can give grounded
// suggestions without the user typing a long instruction.

import { useCallback, useEffect, useMemo, useState } from "react";

import type { LlmMessage, LlmStatus } from "../../../shared/scene";
import { colors, radius, spacing } from "../styles/tokens";

export interface LlmChatPanelProps {
  onStatus: (msg: string | null) => void;
}

type Turn = LlmMessage & { id: string };

// Quick-action kinds are dispatched to distinct bridge endpoints so
// each button actually does what it advertises. The dedicated
// endpoints (`ai.*`) carry the document context server-side; the
// `custom_prompt` variant goes through `llm.chat` with the prompt
// body below for actions that don't have a curated server prompt.
type QuickActionKind =
  | "suggest_for_selection"
  | "layer_naming"
  | "design_tokens"
  | "accessibility"
  | "custom_prompt";

interface QuickAction {
  kind: QuickActionKind;
  label: string;
  /** Used only when `kind === "custom_prompt"`. */
  prompt?: string;
}

const QUICK_ACTIONS: QuickAction[] = [
  {
    kind: "suggest_for_selection",
    label: "Suggest improvements for this design",
  },
  {
    kind: "layer_naming",
    label: "Name my layers",
  },
  {
    kind: "design_tokens",
    label: "Extract design tokens",
  },
  {
    kind: "custom_prompt",
    label: "Generate component variants",
    prompt:
      "For each component in this document, suggest 3 variant names " +
      "(e.g. Default, Hover, Disabled) and one-line behavior notes.",
  },
  {
    kind: "accessibility",
    label: "Find accessibility issues",
  },
];

export function LlmChatPanel({ onStatus }: LlmChatPanelProps): JSX.Element {
  const [status, setStatus] = useState<LlmStatus | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await window.kcreate.llm.status();
      setStatus(s);
    } catch (e) {
      onStatus(`llm status: ${errMsg(e)}`);
    }
  }, [onStatus]);

  useEffect(() => {
    void refreshStatus();
    const id = window.setInterval(() => {
      void refreshStatus();
    }, 3000);
    return () => window.clearInterval(id);
  }, [refreshStatus]);

  const ready = status?.state === "ready";

  const sendMessages = useCallback(
    async (messages: Turn[], userVisibleContent: string) => {
      setSending(true);
      onStatus("LLM: generating…");
      try {
        const reply = await window.kcreate.llm.chat(
          messages.map(({ role, content }) => ({ role, content })),
          512,
          0.2,
        );
        setTurns((prev) => [
          ...prev,
          { id: cryptoId(), role: "user", content: userVisibleContent },
          {
            id: cryptoId(),
            role: "assistant",
            content: reply.content,
          },
        ]);
        onStatus(`LLM: ${reply.tokens_used} tokens (${reply.model}).`);
      } catch (e) {
        onStatus(`LLM failed: ${errMsg(e)}`);
      } finally {
        setSending(false);
      }
    },
    [onStatus],
  );

  const handleSend = useCallback(async () => {
    const trimmed = input.trim();
    if (!trimmed || !ready) return;
    setInput("");
    const next: Turn[] = [
      ...turns,
      { id: cryptoId(), role: "user", content: trimmed },
    ];
    await sendMessages(next, trimmed);
  }, [input, ready, sendMessages, turns]);

  const handleQuickAction = useCallback(
    async (action: QuickAction) => {
      if (!ready || sending) return;
      onStatus(`LLM: ${action.label}…`);
      setSending(true);
      try {
        // Each kind has a distinct happy-path call so the action does
        // what its label says. The fallback below catches the case
        // where the dedicated endpoint returns an error (e.g. no
        // project open, sidecar mid-restart) and replays via the
        // generic chat endpoint so the user still sees a response.
        let assistantContent: string;
        let tokensUsed: number;
        let modelName: string;
        switch (action.kind) {
          case "suggest_for_selection": {
            const reply = await window.kcreate.llm.suggestForSelection();
            assistantContent = reply.content;
            tokensUsed = reply.tokens_used;
            modelName = reply.model;
            break;
          }
          case "layer_naming": {
            const reply = await window.kcreate.ai.suggestLayerNames();
            assistantContent = reply.raw_content;
            tokensUsed = reply.tokens_used;
            modelName = reply.model;
            break;
          }
          case "design_tokens": {
            const reply = await window.kcreate.ai.extractDesignTokens();
            assistantContent = reply.json;
            tokensUsed = reply.tokens_used;
            modelName = reply.model;
            break;
          }
          case "accessibility": {
            const reply = await window.kcreate.ai.checkAccessibility();
            assistantContent = reply.json;
            tokensUsed = reply.tokens_used;
            modelName = reply.model;
            break;
          }
          case "custom_prompt": {
            const prompt = action.prompt ?? action.label;
            const reply = await window.kcreate.llm.chat(
              [{ role: "user", content: prompt }],
              512,
              0.2,
            );
            assistantContent = reply.content;
            tokensUsed = reply.tokens_used;
            modelName = reply.model;
            break;
          }
          default: {
            // Exhaustiveness check: TS narrows `action.kind` to
            // `never` here, so adding a new variant without a case
            // branch becomes a compile error.
            const exhaustive: never = action.kind;
            throw new Error(`unhandled quick action: ${String(exhaustive)}`);
          }
        }
        setTurns((prev) => [
          ...prev,
          { id: cryptoId(), role: "user", content: action.label },
          { id: cryptoId(), role: "assistant", content: assistantContent },
        ]);
        onStatus(`LLM: ${tokensUsed} tokens (${modelName}).`);
      } catch (e) {
        onStatus(`LLM quick action failed: ${errMsg(e)}`);
      } finally {
        setSending(false);
      }
    },
    [onStatus, ready, sending],
  );

  const composedTurns = useMemo(() => turns, [turns]);

  return (
    <section
      style={{
        display: "flex",
        flexDirection: "column",
        gap: spacing.sm,
        marginTop: spacing.md,
      }}
    >
      <header style={headerStyle}>
        <strong>Local LLM</strong>
        <StatusBadge status={status} />
      </header>

      <div style={metaRowStyle}>
        <span>
          Model:{" "}
          <code style={monoStyle}>{status?.model_name ?? "—"}</code>
        </span>
        <span>Network: None</span>
      </div>

      <div style={quickActionsStyle}>
        {QUICK_ACTIONS.map((a) => (
          <button
            key={a.label}
            type="button"
            onClick={() => {
              void handleQuickAction(a);
            }}
            disabled={!ready || sending}
            style={quickActionStyle(!ready || sending)}
          >
            {a.label}
          </button>
        ))}
      </div>

      <div style={chatScrollStyle}>
        {composedTurns.length === 0 ? (
          <p style={emptyHintStyle}>
            Start a conversation or run a quick action. Everything stays
            local.
          </p>
        ) : (
          composedTurns.map((t) => (
            <ChatBubble key={t.id} turn={t} />
          ))
        )}
        {sending ? <p style={emptyHintStyle}>Thinking…</p> : null}
      </div>

      <div style={inputRowStyle}>
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              void handleSend();
            }
          }}
          placeholder={
            ready
              ? "Ask the local assistant…"
              : "Start the model from Model Manager"
          }
          disabled={!ready || sending}
          style={inputStyle(!ready || sending)}
        />
        <button
          type="button"
          onClick={() => {
            void handleSend();
          }}
          disabled={!ready || sending || input.trim().length === 0}
          style={sendButtonStyle(!ready || sending || input.trim().length === 0)}
        >
          Send
        </button>
      </div>
    </section>
  );
}

function ChatBubble({ turn }: { turn: Turn }): JSX.Element {
  const isUser = turn.role === "user";
  return (
    <div
      style={{
        display: "flex",
        justifyContent: isUser ? "flex-end" : "flex-start",
      }}
    >
      <div
        style={{
          maxWidth: "85%",
          padding: `${spacing.xs}px ${spacing.sm}px`,
          background: isUser ? colors.accent : colors.bgSoft,
          color: isUser ? colors.textInverse : colors.text,
          fontSize: 12,
          lineHeight: 1.5,
          borderRadius: radius.card,
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
        }}
      >
        {turn.content}
      </div>
    </div>
  );
}

function StatusBadge({ status }: { status: LlmStatus | null }): JSX.Element {
  const state = status?.state ?? "stopped";
  const color =
    state === "ready"
      ? "#0F766E"
      : state === "starting"
        ? colors.accent
        : state === "error"
          ? "#DC2626"
          : colors.textMuted;
  return (
    <span
      style={{
        fontSize: 10,
        fontWeight: 700,
        textTransform: "uppercase",
        letterSpacing: 0.4,
        color,
      }}
    >
      {state}
    </span>
  );
}

function cryptoId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return Math.random().toString(36).slice(2);
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const headerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  fontSize: 13,
  color: colors.text,
};

const metaRowStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  fontSize: 10,
  color: colors.textMuted,
};

const quickActionsStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
};

function quickActionStyle(disabled: boolean): React.CSSProperties {
  return {
    textAlign: "left",
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 11,
    background: disabled ? colors.bgSoft : colors.bg,
    color: disabled ? colors.textMuted : colors.text,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.card / 2,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}

const chatScrollStyle: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: spacing.xs,
  maxHeight: 240,
  overflowY: "auto",
  padding: spacing.xs,
  border: `1px solid ${colors.border}`,
  borderRadius: radius.card / 2,
  background: colors.bg,
};

const emptyHintStyle: React.CSSProperties = {
  margin: 0,
  fontSize: 11,
  color: colors.textMuted,
};

const inputRowStyle: React.CSSProperties = {
  display: "flex",
  gap: spacing.xs,
};

function inputStyle(disabled: boolean): React.CSSProperties {
  return {
    flex: 1,
    padding: `${spacing.xs}px ${spacing.sm}px`,
    fontSize: 12,
    border: `1px solid ${colors.border}`,
    borderRadius: radius.pill,
    background: disabled ? colors.bgSoft : colors.bg,
    color: colors.text,
  };
}

function sendButtonStyle(disabled: boolean): React.CSSProperties {
  return {
    padding: `${spacing.xs}px ${spacing.md}px`,
    fontSize: 12,
    fontWeight: 600,
    background: disabled ? colors.bgSoft : colors.accent,
    color: disabled ? colors.textMuted : colors.textInverse,
    border: `1px solid ${disabled ? colors.border : colors.accent}`,
    borderRadius: radius.pill,
    cursor: disabled ? "not-allowed" : "pointer",
  };
}

const monoStyle: React.CSSProperties = {
  fontFamily:
    'ui-monospace, SFMono-Regular, Menlo, "Roboto Mono", monospace',
  fontSize: 10,
};
