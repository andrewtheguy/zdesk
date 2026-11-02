"""Read a browser's memory, second by second, out of an Instruments recording.

The recording is `xcrun xctrace record --template "Activity Monitor"
--all-processes` of an iPhone or iPad, and what this reads is its
`activity-monitor-process-live` table, exported on a Mac:

    xcrun xctrace export --input run.trace --xpath \
      '/trace-toc/run[@number="1"]/data/table[@schema="activity-monitor-process-live"]' \
      > live.xml

It prints every one of Safari's page processes (`com.apple.WebKit.WebContent`,
the tab and any other), WebKit's GPU processes and the system's video decoder
(`videocodecd`), each by its process ID, so a process that iOS replaced shows
as another ID: each one's physical footprint, the column Instruments calls
Memory, and its CPU, once a second, then each one's range. The ranges cover the
whole recording, or with `--between` only the seconds the session ran. See
docs/browser-memory.md.

    uv run --python 3.13 python tests/xctrace_memory.py live.xml --between 6 60
"""

import argparse
import collections
import xml.etree.ElementTree as ET

PAGE = "com.apple.WebKit.WebContent"
GPU = "com.apple.WebKit.GPU"
DECODER = "videocodecd"


def rows(path):
    """Each row of the table as {column: (text, fmt)}, its references resolved."""
    root = ET.parse(path).getroot()
    columns = [col.findtext("mnemonic") for col in root.find(".//schema").findall("col")]
    seen = {}

    def value(element):
        if "ref" in element.attrib:
            return seen[element.attrib["ref"]]
        for inner in element.iter():
            if "id" in inner.attrib:
                seen[inner.attrib["id"]] = (inner.text, inner.attrib.get("fmt"))
        return (element.text, element.attrib.get("fmt"))

    for row in root.iter("row"):
        yield {name: value(element) for name, element in zip(columns, list(row))}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("live", help="the exported activity-monitor-process-live table")
    parser.add_argument(
        "--between",
        nargs=2,
        type=int,
        metavar=("FROM", "TO"),
        help="the seconds into the recording the session ran between, which the ranges cover alone",
    )
    args = parser.parse_args()

    # (process, second) -> (MiB, CPU), the second's last sample.
    samples = collections.defaultdict(dict)
    for row in rows(args.live):
        process = row["process"][1]
        footprint = row["memory-physical-footprint"][0]
        if not footprint or not footprint.isdigit():
            continue
        second = round(int(row["start"][0]) / 1e9)
        samples[process][second] = (int(footprint) / 2**20, row["cpu-percent"][1])

    # Every process of each kind, by its process ID.
    kinds = {"page": PAGE, "gpu": GPU, "decoder": DECODER}
    watched = {kind: sorted(p for p in samples if p.startswith(prefix)) for kind, prefix in kinds.items()}
    seconds = sorted({second for processes in watched.values() for p in processes for second in samples[p]})
    for second in seconds:
        cells = []
        for kind, processes in watched.items():
            here = [
                f"{p.rsplit(' ', 1)[-1]} {samples[p][second][0]:6.1f} MiB {samples[p][second][1] or '':>6}"
                for p in processes
                if second in samples[p]
            ]
            if here:
                cells.append(f"{kind} " + ", ".join(here))
        print(f"{second:4d}s  " + "  ".join(cells))

    if args.between:
        start, end = args.between
        span = f"between {start} s and {end} s"
    else:
        start, end = min(seconds, default=0), max(seconds, default=0)
        span = "over the whole recording"
    for kind, processes in watched.items():
        for p in processes:
            footprints = [m for second, (m, _) in samples[p].items() if start <= second <= end]
            if footprints:
                print(f"{kind} {p}: {min(footprints):.1f}–{max(footprints):.1f} MiB {span}")


if __name__ == "__main__":
    main()
