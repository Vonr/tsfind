#!/bin/bash

start=$(grep -m 1 -n '^# Usage$' README.md | sed -E 's/^([0-9]+).*/\1/')
end=$(tail "+$((start + 1))" README.md | grep -m 1 -n '^#' | sed -E "s/^([0-9]+).*/\1+${start}/" | bc -l)
total=$(<README.md wc -l)

if [ -z "$start" ]; then start=$((total + 1)); fi
if [ -z "$end" ]; then end=$((total + 1)); fi

temp=$(mktemp)

cat <<EOF >"$temp"
$(head "-$((start - 1))" README.md)

# Usage

\`\`\`
$(cargo run -q -- --help)
\`\`\`

$(tail "-$((total - end + 1))" README.md)
EOF

mv "$temp" README.md
