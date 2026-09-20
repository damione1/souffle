import re

with open("src-tauri/souffle-slint/lang/fr/LC_MESSAGES/souffle-slint.po", "r", encoding="utf-8") as f:
    content = f.read()

# Replace all empty msgstr with the msgid
def replacer(match):
    msgid = match.group(1)
    if msgid == "":
        return match.group(0) # Keep empty msgstr for header
    return f'msgid "{msgid}"\nmsgstr "{msgid}"'

content = re.sub(r'msgid "(.*?)"\nmsgstr ""', replacer, content)

with open("src-tauri/souffle-slint/lang/fr/LC_MESSAGES/souffle-slint.po", "w", encoding="utf-8") as f:
    f.write(content)
