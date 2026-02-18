import type { Plugin } from "@opencode-ai/plugin"

export const LintFeedbackPlugin: Plugin = async ({ client, $ }) => {
  let filesEdited = false
  let retries = 0
  const MAX_RETRIES = 3

  function toast(message: string, variant: "info" | "success" | "warning" | "error") {
    return client.tui.showToast({
      body: { title: "Lint", message, variant, duration: 3000 },
    })
  }

  return {
    "file.edited": async () => {
      filesEdited = true
    },

    event: async ({ event }) => {
      if (event.type === "session.status") {
        if (event.properties.status.type === "busy") {
          retries = 0
        }
      }

      if (event.type !== "session.idle") return
      if (!filesEdited) return

      if (retries >= MAX_RETRIES) {
        filesEdited = false
        await toast(`Still failing after ${MAX_RETRIES} retries`, "warning")
        return
      }

      const sessionID = event.properties.sessionID
      const result = await $`SKIP=no-commit-to-branch just lint --quiet`.nothrow().quiet()

      if (result.exitCode === 0) {
        filesEdited = false
        return
      }

      retries++
      const output = result.text()

      await toast(`Lint failed (${retries}/${MAX_RETRIES})`, "error")
      await client.session.promptAsync({
        path: { id: sessionID },
        body: {
          parts: [
            {
              type: "text",
              synthetic: true,
              text: [
                `\`just lint\` failed (attempt ${retries}/${MAX_RETRIES}). Fix these issues:`,
                "",
                "```",
                output,
                "```",
              ].join("\n"),
            },
          ],
        },
      })
    },
  }
}
