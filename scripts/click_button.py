import sys
import subprocess

def click_button(name):
    script = f'''
    tell application "System Events"
        tell process "souffle"
            set all_ui to entire contents of group 1 of window 1
            repeat with el in all_ui
                try
                    if (title of el is "{name}") or (name of el is "{name}") then
                        set pos to position of el
                        set sz to size of el
                        return (item 1 of pos) & "," & (item 2 of pos) & "," & (item 1 of sz) & "," & (item 2 of sz)
                    end if
                end try
            end repeat
        end tell
    end tell
    '''
    try:
        out = subprocess.check_output(["osascript", "-e", script], text=True).strip()
        if not out:
            print(f"Button {name} not found.")
            sys.exit(1)
        
        parts = [int(x.strip()) for x in out.split(",")]
        x, y, w, h = parts
        
        center_x = x + w // 2
        center_y = y + h // 2
        
        print(f"Clicking {name} at {center_x}, {center_y}")
        subprocess.check_call(["cliclick", f"c:{center_x},{center_y}"])
    except subprocess.CalledProcessError as e:
        print(f"Failed to find or click {name}: {e}")
        sys.exit(1)

if __name__ == "__main__":
    click_button(sys.argv[1])
