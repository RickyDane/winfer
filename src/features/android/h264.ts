/**
 * Turns the raw H.264 byte stream from `adb screenrecord` (Annex B) into decoded frames:
 * splits it into NAL units, groups them into access units and feeds a WebCodecs VideoDecoder.
 */

const START = [0, 0, 1];

/** Offsets of start codes (00 00 01, possibly preceded by another 00) in `buf`. */
export function startCodes(buf: Uint8Array): number[] {
  const out: number[] = [];
  for (let i = 0; i + 2 < buf.length; i++) {
    if (buf[i] === 0 && buf[i + 1] === 0 && buf[i + 2] === 1) {
      out.push(i > 0 && buf[i - 1] === 0 ? i - 1 : i);
      i += 2;
    }
  }
  return out;
}

const nalType = (nal: Uint8Array) => {
  // Skip the start code (3 or 4 bytes) to the NAL header.
  const header = nal[2] === 1 ? 3 : 4;
  return { type: nal[header] & 0x1f, firstMbZero: (nal[header + 1] & 0x80) !== 0 };
};

const hex = (n: number) => n.toString(16).padStart(2, "0");

/** "avc1.PPCCLL" from an SPS NAL (with start code). */
export function codecFromSps(sps: Uint8Array): string {
  const h = sps[2] === 1 ? 3 : 4;
  return `avc1.${hex(sps[h + 1])}${hex(sps[h + 2])}${hex(sps[h + 3])}`;
}

export class H264Player {
  private pending = new Uint8Array(0);
  private sps: Uint8Array | null = null;
  private pps: Uint8Array | null = null;
  private frame: Uint8Array[] = [];
  private frameKey = false;
  private codec: string | null = null;
  private decoder: VideoDecoder | null = null;
  private timestamp = 0;
  private flushTimer: number | undefined;
  private failed = false;
  private closed = false;
  /** After a decode error: drop frames until the next keyframe, then carry on. */
  private needKey = true;
  private errors = 0;

  constructor(
    private onFrame: (frame: VideoFrame) => void,
    private onError: (message: string) => void,
  ) {}

  static supported() {
    return typeof VideoDecoder !== "undefined";
  }

  /** Bytes as they arrive from adb. */
  push(chunk: Uint8Array) {
    // Data can still arrive after a stop or reconnect; a closed player ignores it.
    if (this.failed || this.closed) return;
    const buf = new Uint8Array(this.pending.length + chunk.length);
    buf.set(this.pending);
    buf.set(chunk, this.pending.length);
    const starts = startCodes(buf);
    if (starts.length === 0) {
      this.pending = buf;
      return;
    }
    // Every NAL but the last is complete; the last one may still be arriving.
    for (let i = 0; i < starts.length - 1; i++) this.nal(buf.subarray(starts[i], starts[i + 1]));
    this.pending = buf.slice(starts[starts.length - 1]);
    // The encoder only sends frames when the screen changes, so the newest NAL may be the last
    // for a while: treat it as complete once nothing more arrives.
    clearTimeout(this.flushTimer);
    this.flushTimer = window.setTimeout(() => {
      if (this.closed) return;
      if (this.pending.length > 4) {
        this.nal(this.pending);
        this.pending = new Uint8Array(0);
      }
      this.emitFrame();
    }, 60);
  }

  private nal(nal: Uint8Array) {
    const { type, firstMbZero } = nalType(nal);
    if (type === 7) {
      this.emitFrame();
      this.sps = nal.slice();
      this.configure();
    } else if (type === 8) {
      this.pps = nal.slice();
    } else if (type === 1 || type === 5) {
      // The first slice of a new picture starts a new frame.
      if (firstMbZero) this.emitFrame();
      if (type === 5) this.frameKey = true;
      this.frame.push(nal.slice());
    } else if (type === 9) {
      this.emitFrame();
    }
  }

  private configure() {
    if (!this.sps) return;
    const codec = codecFromSps(this.sps);
    if (this.decoder && this.codec === codec) return;
    this.decoder?.close();
    this.codec = codec;
    this.createDecoder(codec);
  }

  private createDecoder(codec: string) {
    this.decoder = new VideoDecoder({
      output: (frame) => {
        this.errors = 0;
        this.onFrame(frame);
      },
      // A damaged frame (e.g. cut off in transit) breaks the decoder: start a fresh one and
      // resume at the next keyframe. Only a decoder that keeps failing gives up.
      error: (e) => {
        if (this.closed) return;
        this.errors += 1;
        if (this.errors > 5) {
          this.failed = true;
          this.onError(e.message);
          return;
        }
        this.needKey = true;
        this.createDecoder(codec);
      },
    });
    // No description: the decoder takes Annex B input, parameter sets in band.
    this.decoder.configure({ codec, optimizeForLatency: true });
  }

  private emitFrame() {
    if (this.frame.length === 0 || !this.decoder || this.decoder.state !== "configured" || (this.needKey && !this.frameKey)) {
      this.frame = [];
      this.frameKey = false;
      return;
    }
    this.needKey = false;
    const parts = this.frameKey && this.sps && this.pps ? [this.sps, this.pps, ...this.frame] : this.frame;
    const size = parts.reduce((n, p) => n + p.length, 0);
    const data = new Uint8Array(size);
    let at = 0;
    for (const p of parts) {
      data.set(p, at);
      at += p.length;
    }
    try {
      this.decoder.decode(new EncodedVideoChunk({ type: this.frameKey ? "key" : "delta", timestamp: this.timestamp, data }));
      this.timestamp += 16_666;
    } catch (e) {
      this.failed = true;
      this.onError(String(e));
    }
    this.frame = [];
    this.frameKey = false;
  }

  close() {
    this.closed = true;
    clearTimeout(this.flushTimer);
    try {
      this.decoder?.close();
    } catch {
      // already closed
    }
    this.decoder = null;
  }
}

export const H264_START_CODE = START;
