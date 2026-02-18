#!/bin/bash
set -euo pipefail

INPUT=$(cat)
STOP_HOOK_ACTIVE=$(echo "$INPUT" | jq -r '.stop_hook_active // false')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

MAX_RETRIES=3
COUNTER_FILE="/tmp/claude-lint-${SESSION_ID}"

# Reset counter if this isn't a stop-hook retry
if [ "$STOP_HOOK_ACTIVE" = "false" ]; then
  rm -f "$COUNTER_FILE"
fi

RETRIES=0
if [ -f "$COUNTER_FILE" ]; then
  RETRIES=$(cat "$COUNTER_FILE")
fi

if [ "$RETRIES" -ge "$MAX_RETRIES" ]; then
  rm -f "$COUNTER_FILE"
  echo '{"systemMessage": "Lint still failing after '"$MAX_RETRIES"' retries"}'
  exit 0
fi

OUTPUT=$(SKIP=no-commit-to-branch just lint 2>&1) && EXIT_CODE=0 || EXIT_CODE=$?

if [ "$EXIT_CODE" -eq 0 ]; then
  rm -f "$COUNTER_FILE"
  exit 0
fi

RETRIES=$((RETRIES + 1))
echo "$RETRIES" > "$COUNTER_FILE"

REASON=$(cat <<EOF
\`just lint\` failed (attempt ${RETRIES}/${MAX_RETRIES}). Fix these issues:

\`\`\`
${OUTPUT}
\`\`\`
EOF
)

jq -n --arg reason "$REASON" '{"decision": "block", "reason": $reason}'
