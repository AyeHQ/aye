"""Draw the README's benchmark chart from run.sh's hyperfine exports.

    uv run --no-project --with matplotlib scripts/benchmark/plot.py RESULTS_DIR OUT_DIR

Writes benchmark.svg and benchmark-dark.svg to OUT_DIR.
"""

import json
import os
import platform
import subprocess
import sys
from pathlib import Path

import matplotlib.pyplot as plt
from matplotlib import font_manager
from matplotlib.patches import FancyBboxPatch

THEMES = {
    "light": {
        "surface": "#ffffff",
        "ink": "#0e1210",
        "ink2": "#52514e",
        "muted": "#8a8f8c",
        "grid": "#ebedeb",
        "native": "#c3c7c4",
        "aye": "#0fa968",
    },
    "dark": {
        "surface": "#0d1110",
        "ink": "#f4f5f3",
        "ink2": "#c3c2b7",
        "muted": "#7d837f",
        "grid": "#1f2422",
        "native": "#4a504d",
        "aye": "#2bd47b",
    },
}

# (hyperfine export, framework, what was run, [(command name, label)]). A native
# command sets the baseline for the aye commands after it with the same -n.
PANELS = [
    (
        "pytest",
        "pytest",
        "networkx · 9,978 tests · pytest-xdist for -n auto",
        [
            ("pytest", "pytest"),
            ("aye", "aye"),
            ("pytest -n auto", "pytest -n auto"),
            ("aye -n auto", "aye -n auto"),
        ],
    ),
    (
        "unittest",
        "unittest",
        "sqlglot · 1,552 unit tests · unittest has no -n",
        [
            ("python -m unittest", "python -m unittest"),
            ("aye", "aye"),
            ("aye -n auto", "aye -n auto"),
        ],
    ),
]

# Within 10% either way is "on par": closer than run-to-run noise on a busy machine.
ON_PAR = 1.1
TIMES = "\N{MULTIPLICATION SIGN}"


def font():
    installed = {f.name for f in font_manager.fontManager.ttflist}
    for name in ("Inter", "Helvetica Neue", "Helvetica", "Arial"):
        if name in installed:
            return name
    return "DejaVu Sans"


def machine():
    if platform.system() == "Darwin":
        cpu = subprocess.run(
            ["sysctl", "-n", "machdep.cpu.brand_string"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
    else:
        cpu = platform.processor() or platform.machine()
    return f"{cpu}, {os.cpu_count()} cores"


def load(results, name):
    data = json.loads((results / f"{name}.json").read_text())
    return {r["name"]: r["summary"]["time_wall_clock"]["mean"] for r in data["results"]}


def bar(ax, y, width, height, color, radius=5):
    # Rounded by `radius` pixels at the data end only: the box starts left of the
    # baseline, where the axes clip its other rounded end. Call after setting the limits.
    box = ax.get_window_extent()
    px_per_x = box.width / ax.get_xlim()[1]
    px_per_y = box.height / (ax.get_ylim()[1] - ax.get_ylim()[0])
    rx = radius / px_per_x
    ax.add_patch(
        FancyBboxPatch(
            (-2 * rx, y - height / 2),
            width + 2 * rx,
            height,
            boxstyle=f"round,pad=0,rounding_size={rx}",
            mutation_aspect=px_per_x / px_per_y,
            linewidth=0,
            facecolor=color,
        )
    )


def verdict(baseline, mean):
    ratio = baseline / mean
    if ratio >= ON_PAR:
        return f"{ratio:.1f}{TIMES} faster"
    if ratio > 1 / ON_PAR:
        return "on par"
    return f"{1 / ratio:.1f}{TIMES} slower"


def draw(results, theme, footer):
    t = THEMES[theme]
    plt.rcParams.update({"font.family": font(), "font.size": 11, "svg.fonttype": "path"})
    rows = sum(len(p[3]) for p in PANELS)
    fig, axes = plt.subplots(
        len(PANELS),
        1,
        figsize=(10, 1.6 + 0.46 * rows + 0.55 * len(PANELS)),
        gridspec_kw={"height_ratios": [len(p[3]) for p in PANELS], "hspace": 0.55},
        facecolor=t["surface"],
    )
    fig.subplots_adjust(left=0.27, right=0.86, top=0.80, bottom=0.12)
    fig.text(0.03, 0.94, "aye vs each framework's own runner", fontsize=17, fontweight="bold",
             color=t["ink"])  # fmt: skip
    fig.text(0.03, 0.885, "Wall time for a full run, lower is better. Same interpreter, same "
             f"tests, same outcomes. {machine()}.", fontsize=10.5, color=t["ink2"])  # fmt: skip
    fig.text(0.03, 0.025, footer, fontsize=9, color=t["muted"])

    for ax, (name, framework, about, commands) in zip(axes, PANELS, strict=True):
        means = load(results, name)
        xmax = max(means[c] for c, _ in commands) * 1.02
        ax.set_xlim(0, xmax)
        ax.set_ylim(-0.6, len(commands) - 0.4)
        baselines = {}
        for i, (command, label) in enumerate(commands):
            mean, is_aye, parallel = means[command], command.startswith("aye"), "-n auto" in command
            y = len(commands) - 1 - i
            bar(ax, y, mean, 0.62, t["aye"] if is_aye else t["native"])
            text = f"{mean:.1f} s"
            if not is_aye:
                baselines[parallel] = mean
            elif parallel in baselines:
                text += f"   {verdict(baselines[parallel], mean)}"
            else:
                text += f"   {verdict(baselines[False], mean)} than {commands[0][1]}"
            ax.text(mean + xmax * 0.012, y, text, va="center", fontsize=10.5, color=t["ink"],
                    fontweight="bold" if is_aye else "normal")  # fmt: skip
            ax.text(-xmax * 0.02, y, label, va="center", ha="right", fontsize=10.5,
                    family="monospace", color=t["ink"] if is_aye else t["ink2"])  # fmt: skip
        ax.set_facecolor(t["surface"])
        ax.set_yticks([])
        ax.tick_params(axis="x", colors=t["muted"], length=0, labelsize=9.5)
        ax.xaxis.set_major_formatter(lambda v, _: f"{v:.0f} s")
        ax.grid(axis="x", color=t["grid"], lw=0.8)
        ax.set_axisbelow(True)
        for spine in ax.spines.values():
            spine.set_visible(False)
        ax.text(-0.355, 1.06, framework, transform=ax.transAxes, fontsize=13, fontweight="bold",
                color=t["ink"], va="bottom")  # fmt: skip
        ax.text(1.0, 1.06, about, transform=ax.transAxes, fontsize=10, color=t["ink2"],
                va="bottom", ha="right")  # fmt: skip
    return fig


def main():
    results, out = Path(sys.argv[1]), Path(sys.argv[2])
    footer = (results / "footer.txt").read_text().strip()
    out.mkdir(parents=True, exist_ok=True)
    for theme in THEMES:
        fig = draw(results, theme, footer)
        suffix = "" if theme == "light" else "-dark"
        path = out / f"benchmark{suffix}.svg"
        fig.savefig(path, facecolor=fig.get_facecolor())
        plt.close(fig)
        # Without matplotlib's trailing spaces, as the repository's pre-commit hook keeps files.
        path.write_text("".join(line.rstrip() + "\n" for line in path.read_text().splitlines()))


if __name__ == "__main__":
    main()
