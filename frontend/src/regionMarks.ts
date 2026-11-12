// The outlines of wlshare's marks: where each moving region of its tiles moves,
// and why wlshare has a region of it, for a person looking into what it makes a
// video of.
//
// wlshare sends a mark only with `motion_debug` set in its configuration, and the
// gateway passes each as a `regionMark`: a stream of the tiles' regions
// (regionStreams.ts), a rectangle in framebuffer pixels inside the stream's own,
// the box around the pixels that changed there, and a reason. A mark is none of
// the picture, so it is not drawn on the canvas, where a copy or the page's cache
// of tiles would take it for the desktop's: each is an element over the canvas, in
// a layer the canvas's size that moves as the canvas does, and is gone with its
// stream. A stream's next mark takes the place of its last; its `regionEnd` ends
// it, and so does a picture that starts over or a session's end.

/** Why wlshare has a region, as its last mark said: its tiles kept changing, or one change was more than its stills. */
export type RegionWhy = "moving" | "burst";

/** A mark's rectangle, in framebuffer pixels. */
export interface RegionMark {
  x: number;
  y: number;
  w: number;
  h: number;
  why: RegionWhy;
}

/** Where an outline lies in its layer, in hundredths of the layer's width and height. */
export interface MarkBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * `mark` as a share of a `size` framebuffer, which the layer is stretched over
 * whatever the canvas is scaled to. Null for a mark that is not inside it, or a
 * framebuffer of no size.
 */
export function markBox(
  mark: RegionMark,
  size: { w: number; h: number },
): MarkBox | null {
  if (
    size.w <= 0 ||
    size.h <= 0 ||
    mark.w <= 0 ||
    mark.h <= 0 ||
    mark.x < 0 ||
    mark.y < 0 ||
    mark.x + mark.w > size.w ||
    mark.y + mark.h > size.h
  ) {
    return null;
  }
  return {
    left: (mark.x / size.w) * 100,
    top: (mark.y / size.h) * 100,
    width: (mark.w / size.w) * 100,
    height: (mark.h / size.h) * 100,
  };
}

/** What the outlines are drawn in: one element each. */
export interface MarkLayer<E> {
  add(): E;
  place(el: E, box: MarkBox, why: RegionWhy): void;
  remove(el: E): void;
}

export interface RegionMarks {
  /** Adopt a `regionMark`: the stream's outline is this one from here. */
  set(stream: number, mark: RegionMark): void;
  /** Adopt a `regionEnd`: the stream's outline goes with it. */
  end(stream: number): void;
  /** The framebuffer's size is another, or known at last: every outline lies by it. */
  resized(): void;
  /** The picture starts over, or the session ends: every outline goes. */
  clear(): void;
}

/**
 * The outlines in `layer`, each lying by the framebuffer `size` says the page
 * presents: a mark that came ahead of it, or is not inside it, is kept and shown
 * no outline until `resized` finds it one.
 */
export function createRegionMarks<E>(
  layer: MarkLayer<E>,
  size: () => { w: number; h: number } | null,
): RegionMarks {
  const live = new Map<number, { mark: RegionMark; el?: E }>();
  const show = (entry: { mark: RegionMark; el?: E }) => {
    const framebuffer = size();
    const box = framebuffer ? markBox(entry.mark, framebuffer) : null;
    if (!box) {
      if (entry.el !== undefined) {
        layer.remove(entry.el);
        entry.el = undefined;
      }
      return;
    }
    entry.el ??= layer.add();
    layer.place(entry.el, box, entry.mark.why);
  };
  return {
    set(stream, mark) {
      const entry = live.get(stream) ?? { mark };
      entry.mark = mark;
      live.set(stream, entry);
      show(entry);
    },
    end(stream) {
      const entry = live.get(stream);
      if (entry?.el !== undefined) {
        layer.remove(entry.el);
      }
      live.delete(stream);
    },
    resized() {
      for (const entry of live.values()) {
        show(entry);
      }
    },
    clear() {
      for (const entry of live.values()) {
        if (entry.el !== undefined) {
          layer.remove(entry.el);
        }
      }
      live.clear();
    },
  };
}

/** The layer in the page: `host`'s children, styled by index.css (`.region-mark`). */
export function domMarkLayer(
  host: () => HTMLElement | null,
): MarkLayer<HTMLElement> {
  return {
    add() {
      const el = document.createElement("div");
      host()?.appendChild(el);
      return el;
    },
    place(el, box, why) {
      el.className = `region-mark ${why}`;
      el.style.left = `${box.left}%`;
      el.style.top = `${box.top}%`;
      el.style.width = `${box.width}%`;
      el.style.height = `${box.height}%`;
    },
    remove(el) {
      el.remove();
    },
  };
}
