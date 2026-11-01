// The WebGL canvas the page lays over the desktop's (RemoteDesktop.tsx), as the two
// pictures drawn on it hold it: an RDP host's graphics pipeline (egfxPicture.ts)
// and a software decoder's (planesPicture.ts). Both are pixels in a memory
// WebAssembly threads share, which the desktop's 2D canvas takes no image data
// out of and a WebGL texture takes an upload from as it is.
//
// The canvas outlives a picture: it is the page's, handed to the paint worker
// once, and its WebGL context is the one it will ever have. A picture is what one
// session keeps in that context, and gives back.
//
// The browser may take that context away (iOS Safari does, for a tab it puts in
// the background or short of memory) and give it back. Everything a picture made
// in it is gone then, and a picture is not drawn again: the paint worker learns
// of both from the canvas (`watchPictureContext`), drops the picture when it is
// taken, and makes another when it is given back, filled from what it was drawn
// from.
//
// Without WebGL 2 there is no such picture: the page says so.

// One triangle over the whole canvas, `uv` from its top left corner's (0, 1) down
// to the bottom right's (1, 0).
const VERTEX = `#version 300 es
out vec2 uv;
void main() {
  uv = vec2((gl_VertexID & 1) * 2, gl_VertexID & 2);
  gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}`;

/**
 * What a picture throws once the browser has taken its context away: not the
 * picture failing, since the browser may give the context back.
 */
export class PictureContextLost extends Error {
  constructor() {
    super("the browser took the picture's WebGL context away");
    this.name = "PictureContextLost";
  }
}

/**
 * Hear the browser take `canvas`'s WebGL context away and give it back. Taking
 * it is answered by asking for it back, without which the browser keeps it.
 */
export function watchPictureContext(
  canvas: OffscreenCanvas,
  on: { lost(): void; restored(): void },
) {
  canvas.addEventListener("webglcontextlost", (event) => {
    event.preventDefault();
    on.lost();
  });
  canvas.addEventListener("webglcontextrestored", () => on.restored());
}

export interface PictureCanvas {
  readonly gl: WebGL2RenderingContext;
  /** The picture's program, in use. */
  readonly program: WebGLProgram;
  /** Throws `PictureContextLost` once the browser has taken the context away. */
  usable(): void;
  /**
   * The canvas at this size, its drawing buffer cleared. WebGL reports a refused
   * size as a smaller buffer, not an exception, and a draw into that one is not
   * the picture: so it is checked here, and thrown.
   */
  resize(w: number, h: number): void;
  /** One triangle over the whole canvas, through the program. */
  draw(): void;
  /**
   * Give the program back and leave the canvas black, its context as it was
   * found: what it shows next is another picture's.
   */
  close(): void;
}

function shader(gl: WebGL2RenderingContext, kind: number, source: string) {
  const made = gl.createShader(kind);
  if (!made) {
    throw new Error("WebGL 2 gave no shader");
  }
  gl.shaderSource(made, source);
  gl.compileShader(made);
  return made;
}

/**
 * Take `canvas` for a picture drawn by `fragment`, a fragment shader given `uv`
 * by the vertex shader above.
 */
export function openPictureCanvas(
  canvas: OffscreenCanvas,
  fragment: string,
): PictureCanvas {
  // The canvas's one context: made by the first picture, the same one after.
  const gl = canvas.getContext("webgl2", {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
  });
  if (!gl) {
    throw new Error("WebGL 2 is not available");
  }
  const usable = () => {
    if (gl.isContextLost()) {
      throw new PictureContextLost();
    }
  };
  usable();
  const vertex = shader(gl, gl.VERTEX_SHADER, VERTEX);
  const pixels = shader(gl, gl.FRAGMENT_SHADER, fragment);
  const program = gl.createProgram();
  gl.attachShader(program, vertex);
  gl.attachShader(program, pixels);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    usable();
    throw new Error(
      `WebGL 2 refused the picture's shaders: ${gl.getProgramInfoLog(program) || gl.getShaderInfoLog(pixels)}`,
    );
  }
  gl.useProgram(program);
  const largest = gl.getParameter(gl.MAX_TEXTURE_SIZE) as number;

  return {
    gl,
    program,
    usable,
    resize(w, h) {
      usable();
      if (w > largest || h > largest) {
        throw new Error(
          `the GPU takes no picture over ${largest} pixels a side (this one is ${w}x${h})`,
        );
      }
      canvas.width = w;
      canvas.height = h;
      if (gl.drawingBufferWidth !== w || gl.drawingBufferHeight !== h) {
        usable();
        throw new Error(
          `the GPU gave a ${gl.drawingBufferWidth}x${gl.drawingBufferHeight} canvas for a ${w}x${h} picture`,
        );
      }
      gl.viewport(0, 0, w, h);
    },
    draw() {
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
    close() {
      // Nothing of a lost context's is there to give back. A picture is closed
      // while its context is lost, not after the browser gives it back: its
      // objects are not the restored context's, and deleting one is an error a
      // later picture's allocation check would read.
      if (gl.isContextLost()) {
        return;
      }
      gl.deleteProgram(program);
      gl.deleteShader(vertex);
      gl.deleteShader(pixels);
      // What a picture may have set, as the next one expects to find it.
      gl.activeTexture(gl.TEXTURE0);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
      gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
      gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, 0);
      gl.pixelStorei(gl.UNPACK_SKIP_ROWS, 0);
      if ("drawingBufferColorSpace" in gl) {
        gl.drawingBufferColorSpace = "srgb";
      }
      // One black pixel: a canvas sized to nothing keeps showing its last frame.
      canvas.width = 1;
      canvas.height = 1;
      gl.clearColor(0, 0, 0, 1);
      gl.clear(gl.COLOR_BUFFER_BIT);
    },
  };
}
