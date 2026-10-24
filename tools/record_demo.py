"""Record the README demo from a real gateway, with no hardware and no network.

Starts `openmhs serve --sim` in a throwaway home, runs the commands in SCRIPT against it
with the real CLI, and writes what they printed:

- docs/assets/demo-transcript.txt  the plain-text transcript
- docs/assets/demo.svg             the same transcript as an animated terminal (plays once,
                                   ends on the full transcript; static when animation is off)

The agent token is replaced by `omhs_<redacted>` (a throwaway for a gateway that is stopped
seconds later) and the `export` line is displayed rather than typed; everything else is the
real output of this run, including command ids and timestamps.

Usage: python tools/record_demo.py --bin target/release/openmhs [--out docs/assets]
"""
import argparse
import html
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent

# (shown command, argv after `openmhs`, runs with the agent token?)  A string starting with
# "#" is a comment line, shown and not run.
SCRIPT = [
    "# terminal 1:  openmhs serve --sim",
    "# terminal 2, as the operator: allow an agent to use the power supply",
    ("openmhs devices list", ["devices", "list"], False),
    (
        "openmhs grants add --id agent-psu --principal agent:demo --device sim.psu-01 --capability '*' --ops read,write --effect allow",
        ["grants", "add", "--id", "agent-psu", "--principal", "agent:demo", "--device", "sim.psu-01",
         "--capability", "*", "--ops", "read,write", "--effect", "allow"],
        False,
    ),
    ("openmhs tokens create agent:demo --any-scope", ["tokens", "create", "agent:demo", "--any-scope"], False),
    "# now act as the agent",
    ("openmhs exec sim.psu-01 power.output.voltage.setpoint --value 5V", ["exec", "sim.psu-01", "power.output.voltage.setpoint", "--value", "5V"], True),
    ("openmhs exec sim.psu-01 power.output.voltage.setpoint --value 2V", ["exec", "sim.psu-01", "power.output.voltage.setpoint", "--value", "2V"], True),
    ("openmhs exec sim.psu-01 power.output.voltage.setpoint --value 48V", ["exec", "sim.psu-01", "power.output.voltage.setpoint", "--value", "48V"], True),
    "# and as the operator again",
    ("openmhs audit verify", ["audit", "verify"], False),
]

TOKEN = re.compile(r"omhs_[0-9a-f]{16,}")


def run(binary, home, argv, token):
    env = dict(os.environ, OPENMHS_HOME=str(home), NO_COLOR="1")
    env.pop("OPENMHS_TOKEN", None)
    if token:
        env["OPENMHS_TOKEN"] = token
    p = subprocess.run([binary, *argv], env=env, capture_output=True, text=True, timeout=60)
    return (p.stdout + p.stderr).rstrip("\n"), p.returncode


def start_gateway(binary, home):
    env = dict(os.environ, OPENMHS_HOME=str(home), NO_COLOR="1")
    proc = subprocess.Popen([binary, "serve", "--sim", "--port", "0"], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 30
    while time.time() < deadline:
        if (home / "gateway.json").exists():
            out, rc = run(binary, home, ["devices", "list"], None)
            if rc == 0:
                return proc
        time.sleep(0.3)
    proc.kill()
    raise SystemExit("the gateway did not start")


def record(binary):
    lines = []  # (kind, text)
    with tempfile.TemporaryDirectory() as tmp:
        home = pathlib.Path(tmp) / "home"
        proc = start_gateway(binary, home)
        token = None
        try:
            for item in SCRIPT:
                if isinstance(item, str):
                    lines.append(("comment", item))
                    continue
                shown, argv, as_agent = item
                lines.append(("prompt", "$ " + shown))
                out, rc = run(binary, home, argv, token if as_agent else None)
                m = TOKEN.search(out)
                if argv[:2] == ["tokens", "create"] and m:
                    token = m.group(0)
                out = TOKEN.sub("omhs_<redacted>", out)
                for text in out.splitlines():
                    lines.append(("out", text))
                if argv[:2] == ["tokens", "create"] and token:
                    # The script passes the token in the environment, as this line would.
                    lines.append(("prompt", "$ export OPENMHS_TOKEN=omhs_<redacted>"))
                lines.append(("blank", ""))
        finally:
            proc.kill()
            proc.wait()
    while lines and lines[-1][0] == "blank":
        lines.pop()
    return lines


def classify(kind, text):
    if kind == "prompt":
        return "prompt"
    if kind == "comment":
        return "comment"
    low = text.lower()
    if "denied" in low or low.startswith("error"):
        return "bad"
    if "verified" in low or "intact" in low or low.startswith("added") or low.startswith("created"):
        return "good"
    return "out"


def svg(lines):
    fs, lh, pad_x, pad_top, width = 12, 17, 18, 44, 900
    t = 0.4
    body, styles = [], []
    for i, (kind, text) in enumerate(lines):
        cls = classify(kind, text)
        y = pad_top + (i + 1) * lh
        if kind == "prompt":
            t += 0.9
            gap = 0.0
        elif kind == "comment":
            t += 0.6
        elif kind == "blank":
            t += 0.15
        else:
            t += 0.07
        body.append(
            f'<text class="l {cls}" style="animation-delay:{t:.2f}s" x="{pad_x}" y="{y}" xml:space="preserve">{html.escape(text)}</text>'
        )
    height = pad_top + (len(lines) + 1) * lh + 16
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" role="img" aria-label="OpenMHS terminal demo: an agent is refused a 5 V jump and 48 V, a 2 V setpoint is confirmed by readback, and the audit log verifies">
<style>
.bg{{fill:#0d1117}} .bar{{fill:#161b22}}
.l{{font:{fs}px ui-monospace,SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace;white-space:pre;fill:#c9d1d9;opacity:0;animation:show .01s forwards}}
.prompt{{fill:#e6edf3;font-weight:600}} .comment{{fill:#7d8590}} .bad{{fill:#ff7b72}} .good{{fill:#7ee787}}
@keyframes show{{to{{opacity:1}}}}
@media (prefers-reduced-motion:reduce){{.l{{opacity:1;animation:none}}}}
</style>
<rect class="bg" width="{width}" height="{height}" rx="8"/>
<rect class="bar" width="{width}" height="28" rx="8"/><rect class="bar" y="14" width="{width}" height="14"/>
<circle cx="18" cy="14" r="5" fill="#ff5f56"/><circle cx="36" cy="14" r="5" fill="#ffbd2e"/><circle cx="54" cy="14" r="5" fill="#27c93f"/>
<text x="{width // 2}" y="18" text-anchor="middle" style="font:11px ui-monospace,Menlo,Consolas,monospace;fill:#7d8590">openmhs · simulated devices only</text>
{chr(10).join(body)}
</svg>
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=shutil.which("openmhs") or "openmhs")
    ap.add_argument("--out", default=str(ROOT / "docs" / "assets"))
    args = ap.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    binary = args.bin
    if pathlib.Path(binary).is_file():
        binary = str(pathlib.Path(binary).resolve())
    lines = record(binary)
    transcript = "\n".join(t for _, t in lines) + "\n"
    (out / "demo-transcript.txt").write_text(
        "# Recorded by tools/record_demo.py from a real `openmhs serve --sim` gateway.\n"
        "# Simulated devices only. The agent token is redacted, and the `export` line is shown\n"
        "# rather than typed (the script sets the variable). All other output is verbatim.\n\n" + transcript,
        encoding="utf-8", newline="\n")
    (out / "demo.svg").write_text(svg(lines), encoding="utf-8", newline="\n")
    print(f"wrote {out / 'demo-transcript.txt'} and {out / 'demo.svg'} ({len(lines)} lines)")


if __name__ == "__main__":
    sys.exit(main())
