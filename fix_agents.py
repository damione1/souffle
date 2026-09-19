import re

with open('AGENTS.md', 'r') as f:
    content = f.read()

content = re.sub(r"`#\[derive\(specta::Type\)\]`, reaching the frontend through the generated client\n\(`src/lib/types/generated\.ts`\)\. The frontend imports it\. It never retypes it\.\n", "", content)
content = re.sub(r"- Never hand-write `impl specta::Type` to flatten an enum into `String`\. That silently deletes the\n  contract for that type and forces the frontend to invent it again\.\n", "", content)

with open('AGENTS.md', 'w') as f:
    f.write(content)

with open('CLAUDE.md', 'r') as f:
    content = f.read()

content = content.replace("No webview, no npm frontend in the shipped app.", "No webview in the shipped app.")

with open('CLAUDE.md', 'w') as f:
    f.write(content)
