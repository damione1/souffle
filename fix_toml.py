import re

def remove_dep(file, dep):
    with open(file, 'r') as f:
        content = f.read()
    content = re.sub(r"^" + dep + r"\s*=.*?\n", "", content, flags=re.MULTILINE)
    with open(file, 'w') as f:
        f.write(content)

remove_dep('src-tauri/Cargo.toml', 'candle-transformers')
remove_dep('src-tauri/Cargo.toml', 'log')
remove_dep('src-tauri/Cargo.toml', 'sha2')
remove_dep('src-tauri/souffle-mcp/Cargo.toml', 'chrono')

