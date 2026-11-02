# Memory in the browser

What a session costs an iPhone in memory under each way of drawing an RDP host's
pipeline, measured to decide which to suggest on a phone (issue #720). The design
these figures bear on is in
[RDP's graphics pipeline, passed through](architecture.md#rdps-graphics-pipeline-passed-through).

## The runs

zDesk 0.0.506 on zdesk-1, 2026-11-02. The target was `windowselite`, a Windows host
whose desktop is 3840×1960 at 2x, over one display. The browser was Safari on an
iPhone 15 Pro (A17 Pro, 8 GB) running iOS 26.7.2. Each run is a recording of about
two minutes: sessions connected, a video played on the host, and the session
was disconnected and connected again. The H.264 region run switched its video between
a window and full screen.

| Picker choice | Safari's page process | WebKit's GPU process | `videocodecd` | Sum of the peaks | Page CPU |
|---|---|---|---|---|---|
| Adaptive, H.264 full screen | 283 MiB | 210 MiB | 62 MiB | 555 MiB | under 15% of a core |
| High quality | 734 MiB | 114 MiB | 5 MiB, unused | 853 MiB | 120–155% |
| Adaptive, H.264 region | 869 MiB | 187 MiB | 52 MiB | 1,108 MiB | 25–145% |

Each is the process's peak physical footprint, the column Instruments calls Memory,
over the recording, as `tests/xctrace_memory.py` reports it; the page's CPU is its
range while a session streamed. With no session the page process held 45–115 MiB,
the GPU process about 26 MiB and `videocodecd` 5 MiB. Once its picture had come,
full screen's page process stayed between about 190 and 283 MiB.

- **Full screen costs least**, about 300 MiB less than High quality, because nothing is
  composed in the page: no compositor, no surface and no host cache. The phone's video
  decoder does the work in `videocodecd` and the GPU process, which is also why the
  page's CPU is a fraction of the other two.
- **The region costs most**: it composes everything High quality composes, and adds
  the video decoder for the parts drawn in H.264.
- **The composed modes climb.** Through each High quality and region session the page
  process grew, from 575 to 734 MiB and from 641 to 854 MiB within about 40 s, where
  full screen's rose and fell within its range. Two minutes do not show whether it
  levels off.
- **A composed page keeps its memory.** After its last session the High quality page
  held 173 MiB, against 47 MiB before; full screen's went from 89 to 114 MiB.
- **None of it pressed on the phone.** The device used 5.2–6.0 GB in all three runs,
  its process count held steady, and no page process was replaced, so iOS killed
  nothing.

Not measured: sessions longer than two minutes, two displays, an iPad, other
devices, and whether the WebGL context loss of #717 comes with the composed modes'
growth.

## How it was measured

On a Mac with Xcode, the device on USB, Developer Mode on, Auto-Lock at Never, and only
the zDesk tab open in Safari:

```sh
xcrun xctrace list devices
xcrun xctrace record --template "Activity Monitor" --all-processes \
  --device <UDID> --time-limit 2m --output run.trace
```

Start the recording, then the session, noting the time so the gateway log's
`connecting to target` lines can be matched with it. Let the time limit end the
recording, or press Ctrl+C once, and wait for `Output file saved as`: a second Ctrl+C,
or copying the trace before that, leaves only its raw data, and the per-process
tables export empty.

Then export the per-process table, on any Mac with Xcode, and read it:

```sh
xcrun xctrace export --input run.trace --xpath \
  '/trace-toc/run[@number="1"]/data/table[@schema="activity-monitor-process-live"]' \
  > live.xml
uv run --python 3.13 python tests/xctrace_memory.py live.xml
```

`tests/xctrace_memory.py` prints every page process, GPU process and `videocodecd`
once a second, each by its process ID, then each one's range: over the whole
recording, or with `--between FROM TO` over those seconds alone, the ones a session
ran. Whether the phone was short of memory is in the `activity-monitor-system`
table: its `memory-physical-used`, and `total-processes`, which falls when iOS kills
processes.

A process missing from the per-process table for a while has not shown that it is
idle: the table samples a process when it changes, so an idle one leaves a gap, and
so does one that exited. Tell them apart by the process ID: an idle process comes
back under the same one, and a page process iOS killed is followed by another
under a new one, as Safari reloads the tab. A process that is never sampled again
may have done either; `total-processes` is the only other sign.
