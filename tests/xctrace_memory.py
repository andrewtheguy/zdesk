"""Read a browser's memory, second by second, out of an Instruments recording.

The recording is `xcrun xctrace record --template "Activity Monitor"
--all-processes` of an iPhone or iPad, and what this reads is its
`activity-monitor-process-live` table, exported on a Mac:

    xcrun xctrace export --input run.trace --xpath \
      '/trace-toc/run[@number="1"]/data/table[@schema="activity-monitor-process-live"]' \
      > live.xml

It prints Safari's page process (the `com.apple.WebKit.WebContent` with the
largest footprint, which is the tab), WebKit's GPU process and the system's
video decoder (`videocodecd`): each one's physical footprint, the column
Instruments calls Memory, and its CPU, once a second, then their ranges. See
docs/browser-memory.md.

    uv run --python 3.13 python tests/xctrace_memory.py live.xml
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

    def largest(prefix):
        named = [p for p in samples if p.startswith(prefix)]
        return max(named, key=lambda p: max(m for m, _ in samples[p].values()), default=None)

    watched = {"page": largest(PAGE), "gpu": largest(GPU), "decoder": largest(DECODER)}
    print("  ".join(f"{role}: {process}" for role, process in watched.items() if process))
    seconds = sorted({s for p in watched.values() if p for s in samples[p]})
    for second in seconds:
        cells = []
        for role, process in watched.items():
            sample = samples[process].get(second) if process else None
            cells.append(f"{role} {sample[0]:6.1f} MiB {sample[1] or '':>6}" if sample else f"{role} {'':17}")
        print(f"{second:4d}s  " + "  ".join(cells))
    for role, process in watched.items():
        if process:
            footprints = [m for m, _ in samples[process].values()]
            print(f"{role}: {min(footprints):.1f}–{max(footprints):.1f} MiB")


if __name__ == "__main__":
    main()
