#!/usr/bin/env python3
"""Render the README latency chart from Prompt latency workflow results.

    uv run --no-project scripts/latency_chart.py LINUX.json MACOS.json OUTPUT.svg

Inputs are the results.json artifacts of one workflow run. The chart shows p50 for the
large workload, uses the same percentile as prompt_latency.py, and themes itself with
prefers-color-scheme so it reads on both GitHub themes.
"""

import json
import math
import re
import sys
from html import escape

from prompt_latency import percentile

TOOLS = ("capsule", "starship")
RUNNERS = (("Linux", sys.argv[1]), ("macOS", sys.argv[2]))
OUT = sys.argv[3]

reports = {name: json.loads(open(path).read()) for name, path in RUNNERS}


def p50(report, tool, metric):
    large = next(w for w in report["workloads"] if w["name"] == "large")
    return round(percentile([s[metric] for s in large["tools"][tool]], 0.5), 1)


PANELS = [
    ("Input blocked", "time until zsh accepts typing again", "ready_ms"),
    ("Fresh branch shown", "time until the new branch name is on screen", "fresh_ms"),
]
PANELS = [(title, subtitle, {name: tuple(p50(reports[name], tool, metric) for tool in TOOLS) for name, _ in RUNNERS})
          for title, subtitle, metric in PANELS]
MAX = math.ceil(max(v for _, _, groups in PANELS for values in groups.values() for v in values) / 25 + 0.5) * 25
linux, macos = reports["Linux"], reports["macOS"]
files = next(w["files"] for w in linux["workloads"] if w["name"] == "large")
plain = max(p50(r, "plain", "ready_ms") for r in reports.values())


def version(report, tool):
    return re.sub(r"^\S+\s+", "", report["environment"][tool]).split()[0]


def runner(report):
    cpu = report["environment"]["cpu"]
    return f"{cpu} ({report['environment']['cpus']} CPUs)"

W = 760
LEFT = 96          # row-label column
PLOT = 540         # 0..MAX ms
BAR = 18
GAP = 2            # surface gap between touching bars
GROUP_GAP = 16
R = 4              # rounded data-end


def x(ms):
    return LEFT + ms / MAX * PLOT


def bar(x0, y, width, cls):
    r = min(R, width / 2)
    x1 = x0 + width
    return (f'<path class="{cls}" d="M{x0:.1f},{y} H{x1 - r:.1f} '
            f'Q{x1:.1f},{y} {x1:.1f},{y + r} V{y + BAR - r} '
            f'Q{x1:.1f},{y + BAR} {x1 - r:.1f},{y + BAR} H{x0:.1f} Z"/>')


parts = []
y = 28
parts.append(f'<text class="title" x="24" y="{y}">Prompt latency after Enter, {files:,}-file repository</text>')
y += 20
parts.append(f'<text class="muted" x="24" y="{y}">p50 of {linux["rounds"]} interleaved samples on GitHub-hosted runners · lower is better</text>')
y += 26
# Legend
lx = 24
for index, tool in enumerate(TOOLS):
    parts.append(f'<rect class="s{index + 1}" x="{lx}" y="{y - 10}" width="12" height="12" rx="3"/>')
    parts.append(f'<text class="secondary" x="{lx + 18}" y="{y}">{tool}</text>')
    lx += 18 + len(tool) * 8 + 24
y += 36

for title, subtitle, groups in PANELS:
    parts.append(f'<text class="panel" x="24" y="{y}">{title}</text>')
    y += 18
    parts.append(f'<text class="muted" x="24" y="{y}">{escape(subtitle)}</text>')
    y += 16
    top = y
    height = len(groups) * (2 * BAR + GAP) + (len(groups) - 1) * GROUP_GAP
    for tick in range(0, int(MAX) + 1, 25):
        parts.append(f'<line class="grid" x1="{x(tick):.1f}" x2="{x(tick):.1f}" y1="{top - 2}" y2="{top + height + 2}"/>')
    for name, values in groups.items():
        parts.append(f'<text class="secondary" x="{LEFT - 12}" y="{y + BAR + GAP / 2 + 4}" text-anchor="end">{name}</text>')
        for index, (tool, value) in enumerate(zip(TOOLS, values)):
            by = y + index * (BAR + GAP)
            parts.append(bar(LEFT, by, value / MAX * PLOT, f"s{index + 1}"))
            parts.append(f'<text class="value" x="{x(value) + 6:.1f}" y="{by + BAR - 5}">{value:g} ms</text>')
        y += 2 * BAR + GAP + GROUP_GAP
    y = top + height
    parts.append(f'<line class="axis" x1="{LEFT}" x2="{LEFT}" y1="{top - 2}" y2="{y + 2}"/>')
    y += 16
    for tick in range(0, int(MAX) + 1, 25):
        label = f"{tick} ms" if tick == MAX else str(tick)
        parts.append(f'<text class="muted" x="{x(tick):.1f}" y="{y}" text-anchor="middle">{label}</text>')
    y += 30

y -= 6
parts.append(f'<text class="muted" x="24" y="{y}">Default configurations · capsule {version(linux, "capsule")}, '
             f'starship {version(linux, "starship")} · plain zsh prompt: {plain:g} ms or less</text>')
y += 17
parts.append(f'<text class="muted" x="24" y="{y}">{escape("Linux: " + runner(linux) + " · macOS: " + runner(macos))}</text>')
H = y + 20

style = """
.bg{fill:#fcfcfb}
.title{font:600 17px system-ui,-apple-system,"Segoe UI",sans-serif;fill:#0b0b0b}
.panel{font:600 14px system-ui,-apple-system,"Segoe UI",sans-serif;fill:#0b0b0b}
.secondary{font:13px system-ui,-apple-system,"Segoe UI",sans-serif;fill:#52514e}
.value{font:12px ui-monospace,SFMono-Regular,Menlo,monospace;fill:#52514e}
.muted{font:11.5px system-ui,-apple-system,"Segoe UI",sans-serif;fill:#6f6e69}
.grid{stroke:#e9e8e3;stroke-width:1}
.axis{stroke:#b9b8b1;stroke-width:1}
.s1{fill:#2a78d6}.s2{fill:#eb6834}
@media (prefers-color-scheme: dark){
.bg{fill:#1a1a19}
.title,.panel{fill:#f0efec}
.secondary,.value{fill:#c3c2b7}
.muted{fill:#9a998f}
.grid{stroke:#2c2c2a}
.axis{stroke:#4a4a46}
.s1{fill:#3987e5}.s2{fill:#d95926}
}
"""

desc = " ".join(
    f"{title} p50: " + "; ".join(f"{name} " + ", ".join(f"{tool} {v:g} ms" for tool, v in zip(TOOLS, values))
                                for name, values in groups.items()) + "."
    for title, _, groups in PANELS)
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" '
       f'aria-labelledby="t d"><title id="t">Prompt latency: capsule vs starship</title><desc id="d">{desc}</desc>'
       f'<style>{style}</style><rect class="bg" width="{W}" height="{H}" rx="12"/>' + "".join(parts) + "</svg>\n")
open(OUT, "w").write(svg)
