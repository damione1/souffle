import re

files = [
    "AGENTS.md",
    "CLAUDE.md",
    ".coderabbit.yaml",
    "site/src/_data/en.json",
    "site/src/_data/fr.json",
    "DESIGN.md"
]

for f in files:
    try:
        with open(f, 'r') as file:
            content = file.read()
            
        # Specific replacements for CLAUDE.md
        content = re.sub(r"- Retired, still present pending SOU-192 AC8.*?\n", "", content)
        content = re.sub(r"Until SOU-192 AC8 removes `src/`/`package.json`, also run the retired frontend's own checks if you touched anything under `src/`: `npm ci && npm test && npm run check`\. Nothing in the shipped app depends on their result anymore, but `contracts\.yml` still runs them on every push/PR\.\n", "", content)
        content = re.sub(r"No Tauri, no webview, no npm frontend in the shipped app\.", "No webview, no npm frontend in the shipped app.", content)
        content = re.sub(r"`src/` \(Svelte 5\) and `package\.json` are the retired Tauri-era frontend, kept only until the whole SOU-185 epic \(Slint migration\) is verified end to end on a real machine; nothing in the shipped app reads them anymore\. Removing them is tracked in SOU-192 \(AC8\) and is a separate, deliberate step\. Don't delete them as a drive-by cleanup\.\n", "", content)

        # AGENTS.md
        content = re.sub(r"\(or the retired\n\s*IPC\)", "", content)
        content = re.sub(r"\*\*Retired TypeScript.*?never retypes it\.\n\n", "", content)
        content = re.sub(r"- After changing any `\#\[tauri::command\]` signature or any exposed type, run\n\s*`npm run generate:types` and commit both generated files\. CI fails otherwise\.\n", "", content)
        content = re.sub(r"\| Exhaustive `Record` over a generated union \(retired TS\) \| `src/lib/features/meeting/system-audio\.ts` \|\n", "", content)
        content = re.sub(r"\| Exhaustive `switch`, no `default` \(retired TS\) \| `src/lib/stores/app\.svelte\.ts` \(`deriveRuntimePhase`\) \|\n", "", content)
        content = re.sub(r"\| Typed registry, impossible to mistype \(retired TS\) \| `src/lib/features/settings/anchors\.ts` \|\n", "", content)
        content = re.sub(r"\| Typed events and commands \(retired TS\) \| `src-tauri/src/app_events\.rs`, `specta_builder\(\)` in `src-tauri/src/lib\.rs` \|\n", "", content)
        content = re.sub(r"\*\*TypeScript\*\*\n\n.*?\n\n", "", content, flags=re.DOTALL)
        content = re.sub(r"- A list of options, bounds or defaults copied from Rust into a `\.slint`, `\.svelte` or `\.ts` file\.", "- A list of options, bounds or defaults copied from Rust into a `.slint` file.", content)
        content = re.sub(r"- A generated file edited by hand\.\n", "", content)
        content = re.sub(r" \(or the retired IPC\)", "", content)

        # coderabbit
        content = re.sub(r"\s*- \"!src/lib/types/generated\.ts\"", "", content)
        content = re.sub(r"\s*- \"!src/lib/api/generated\.ts\"", "", content)
        content = re.sub(r"\s*- \"!src-tauri/gen/\*\*\"", "", content)
        content = re.sub(r"Tauri IPC boundary\. Flag every new or modified `Result<_, String>`: the frontend cannot\n\s*discriminate a cause from a string, and a typed error already exists\.\n", "IPC boundary. Flag every new or modified `Result<_, String>`: the frontend cannot\ndiscriminate a cause from a string, and a typed error already exists.\n", content)
        content = re.sub(r"A changed \#\[tauri::command\] signature requires `npm run generate:types`; if the\n\s*generated files are not in the diff, say so\.\n", "", content)
        
        # Site data
        content = content.replace("npm run tauri dev", "make nightly")

        # DESIGN.md
        content = content.replace("The old Tauri pill window", "The old pill window")

        with open(f, 'w') as file:
            file.write(content)
    except Exception as e:
        print(f"Error on {f}: {e}")
