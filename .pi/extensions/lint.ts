import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
import { keyHint, ToolExecutionComponent } from "@mariozechner/pi-coding-agent";
import { Text, type TUI } from "@mariozechner/pi-tui";

const stubTui = { requestRender() { } } as unknown as TUI;

// eslint-disable-next-line no-control-regex
const ANSI_RE = /\x1b\[[0-9;]*m/g;
function stripAnsi(s: string) {
  return s.replace(ANSI_RE, "");
}

export default function (pi: ExtensionAPI) {
  let retries = 0;
  const MAX_RETRIES = 3;

  const lintToolDef = {
    name: "lint",
    label: "Lint",
    description: "Run project linter",
    parameters: {},
    async execute() {
      return { content: [{ type: "text" as const, text: "" }], details: {} };
    },
    renderCall(_args: any, theme: any) {
      return new Text(theme.fg("toolTitle", theme.bold("lint")), 0, 0);
    },
    renderResult(
      result: any,
      { expanded, isPartial }: { expanded: boolean; isPartial: boolean },
      theme: any
    ) {
      const passed = result.details?.passed ?? false;

      if (passed) {
        return new Text(theme.fg("success", "passed ✓"), 0, 0);
      }

      let text = theme.fg("error", "failed") +
        theme.fg("muted", ` (${result.details?.attempt}/${result.details?.maxRetries})`);

      if (expanded) {
        const output = result.content?.[0];
        if (output?.type === "text" && output.text) {
          text += "\n\n" + output.text;
        }
      } else if (!isPartial) {
        text += " " + theme.fg("dim", keyHint("expandTools", "to expand"));
      }

      return new Text(text, 0, 0);
    },
  };

  pi.registerMessageRenderer(
    "lint-feedback",
    (message, { expanded }, _theme) => {
      const tool = new ToolExecutionComponent(
        "lint",
        {},
        undefined,
        lintToolDef as any,
        stubTui
      );

      const passed = (message.details as any)?.passed ?? false;

      tool.updateResult(
        {
          content: [{ type: "text", text: passed ? "" : (message.content as string) }],
          details: message.details as any,
          isError: !passed,
        },
        false
      );

      tool.setExpanded(expanded);
      return tool;
    }
  );

  pi.on("agent_start", async () => {
    retries = 0;
  });

  pi.on("agent_end", async (event, ctx) => {
    const touchedFiles = event.messages.some(
      (m) =>
        m.role === "toolResult" &&
        (m.toolName === "write" || m.toolName === "edit")
    );
    if (!touchedFiles) return;

    if (retries >= MAX_RETRIES) {
      ctx.ui.notify(
        `Lint still failing after ${MAX_RETRIES} retries, giving up`,
        "warning"
      );
      return;
    }

    ctx.ui.setStatus("lint", "Running lint...");
    const result = await pi.exec("bash", ["-c", "export SKIP=no-commit-to-branch && just lint"], {
      timeout: 120_000,
    });
    ctx.ui.setStatus("lint", undefined);

    if (result.code === 0) {
      pi.sendMessage({
        customType: "lint-feedback",
        content: "",
        display: true,
        details: { passed: true },
      });
      return;
    }

    retries++;
    const output = stripAnsi((result.stdout + "\n" + result.stderr).trim());

    pi.sendMessage(
      {
        customType: "lint-feedback",
        content: output,
        display: true,
        details: { attempt: retries, maxRetries: MAX_RETRIES, passed: false },
      },
      { triggerTurn: true }
    );
  });
}
