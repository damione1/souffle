import re

with open('src-tauri/souffle-slint/src/lists_ui.rs', 'r') as f:
    content = f.read()

content = re.sub(r"#\[allow\(dead_code\)\]\n    pub\(crate\) fn set_snippet_editing.*?\}\n    \}\n", "", content, flags=re.DOTALL)

with open('src-tauri/souffle-slint/src/lists_ui.rs', 'w') as f:
    f.write(content)

