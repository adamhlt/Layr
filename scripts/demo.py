"""Records the demo video and GIF of Layr annotating an Intel 8086 die.

    python scripts/demo.py [--version 1.2]

Rune's agent tooling plays a scripted session in the debug build, without a window, and
streams the frames to ffmpeg: the same clicks, drags and typing give the same video every time.
Needs cargo and ffmpeg on the PATH. Writes target/demo/demo.mp4 and demo.gif, outside git:
the README shows them from GitHub's CDN (drop them in an issue comment to get their links).
"""

import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORK = os.path.join(ROOT, "target", "demo")
SIZE, FPS = "1400x900", 30
# The README's before/after picture, on GitHub's CDN since it left the repository.
BEFORE_AFTER = "https://github.com/user-attachments/assets/b50888ba-1eba-4eea-95b7-de83e8dbfea4"


def run(cmd, **kw):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True, **kw)


def script(die):
    """The session, as Rune script events. Positions are window points for a 1400 × 900
    window, where the die fits at 113 %."""
    ev, t = [], 0.8

    def at(e, then=0.0):
        nonlocal t
        e["t"] = round(t, 2)
        ev.append(e)
        t += then

    at({"drop_files": {"paths": [die], "at": [700, 450], "hover": 0.6}}, 1.6)

    def box(tool, x, y, dx, dy, colour, extra=()):
        at({"type": "key", "name": tool}, 0.35)
        at({"type": "drag", "x": x, "y": y, "dx": dx, "dy": dy, "dur": 0.7}, 1.0)
        at({"type": "click", "key": colour}, 0.4)
        # A thicker outline than the default reads better in a video.
        at({"type": "drag", "key": "stroke-width", "dx": -80, "dy": 0, "dur": 0.35}, 0.6)
        for k in extra:
            at({"type": "click", "key": k}, 0.4)
        at({"type": "key", "name": "Escape"}, 0.5)

    box("r", 421, 115, 181, 293, "stroke-3")  # bus interface unit, yellow
    box("r", 421, 422, 181, 360, "stroke-4")  # execution unit, cyan
    box("h", 863, 513, 306, 273, "stroke-2", ["hatch-color-2"])  # microcode ROM, magenta
    at({"type": "key", "name": "n"}, 0.35)
    for y in (125, 167, 209):  # bond pads
        at({"type": "click", "x": 1175, "y": y}, 0.45)
        at({"type": "click", "key": "fill-5"}, 0.3)
        at({"type": "key", "name": "Escape"}, 0.3)
    t += 0.3
    at({"type": "key", "name": "v"}, 0.3)
    at({"type": "key", "name": "g"}, 1.0)
    at({"type": "drag", "x": 440, "y": 805, "dx": 220, "dy": -20, "dur": 0.6}, 1.0)
    at({"type": "click", "key": "caption"}, 0.25)
    at({"type": "key", "name": "a", "cmd": True}, 0.2)
    at({"type": "type", "text": "Intel 8086", "every": 0.06}, 0.9)
    for i, label in enumerate(["Bus interface unit", "Execution unit", "Microcode ROM", "Bond pads"]):
        at({"type": "click", "key": f"legend-{i}"}, 0.25)
        at({"type": "key", "name": "a", "cmd": True}, 0.2)
        at({"type": "type", "text": label, "every": 0.05}, len(label) * 0.05 + 0.35)
    at({"type": "key", "name": "Escape"}, 0.3)
    at({"type": "click", "key": "fit"}, 1.5)
    # Export: select the area around the ROM, then back out.
    at({"type": "click", "key": "export"}, 1.2)
    at({"type": "click", "key": "export-area-1"}, 1.0)
    at({"type": "drag", "x": 690, "y": 470, "dx": 420, "dy": 360, "dur": 0.9}, 1.6)
    at({"type": "click", "key": "crop-done"}, 2.2)
    at({"type": "click", "key": "export-cancel"}, 0.5)
    at({"type": "key", "name": "Escape"}, 2.5)
    return ev, t


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--version", default="1.2", help="version shown in the status bar")
    p.add_argument("--photo", default=BEFORE_AFTER, help="the before/after picture (file or URL) the die is cut from")
    args = p.parse_args()
    os.makedirs(WORK, exist_ok=True)

    # The die photo: the "Before" half of the README's before/after picture.
    die = os.path.join(WORK, "die.png")
    run(["ffmpeg", "-v", "error", "-y", "-i", args.photo, "-vf", "crop=762:743:25:57", die])

    events, duration = script(die)
    script_path = os.path.join(WORK, "demo.json")
    with open(script_path, "w") as f:
        json.dump(events, f)

    env = dict(os.environ, LAYR_VERSION=args.version, LAYR_NO_UPDATE_CHECK="1")
    run(["cargo", "build", "--locked"], cwd=ROOT, env=env)
    exe = os.path.join(ROOT, "target", "debug", "layr.exe" if sys.platform == "win32" else "layr")
    raw = os.path.join(WORK, "demo-raw.mp4")
    run([exe, "--script", script_path, "--video", raw, "--duration", f"{duration:.1f}", "--fps", str(FPS),
         "--size", SIZE, "--scale", "1", "--pointer", "--keys", "bottom-right"], env=env)

    out = WORK
    run(["ffmpeg", "-v", "error", "-y", "-i", raw, "-c:v", "libx264", "-crf", "20", "-preset", "slow",
         "-pix_fmt", "yuv420p", "-movflags", "+faststart", os.path.join(out, "demo.mp4")])
    # Bayer dithering and redrawing only what changed keep the detailed photo small as a GIF.
    gif_filter = ("fps=12,scale=960:-1:flags=lanczos,split[a][b];"
                  "[a]palettegen=max_colors=160:stats_mode=diff[p];"
                  "[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle")
    run(["ffmpeg", "-v", "error", "-y", "-i", raw, "-vf", gif_filter, os.path.join(out, "demo.gif")])
    for name in ("demo.mp4", "demo.gif"):
        path = os.path.join(out, name)
        print(f"{path}: {os.path.getsize(path) / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
