"""Live SSTV off the air for the bench: the Xeon's bladeRF streams MHZ (480 kS/s, tuned 120 kHz above, the signal
120 kHz above the centre once I and Q are swapped, as bladecap.sh's captures show it) over ssh;
here it is mixed down, averaged to 24 kS/s (20:1), FM-demodulated and fed to usstv's `live` example, which writes
the picture as it grows (OUT.bmp; watch it with the page `--serve` writes). Runs SECONDS, or until stopped.

  sstvlive.py MHZ SECONDS OUT.bmp [--vis N] [--gain DB] [--serve PORT]
"""
import argparse, os, subprocess, sys, threading, http.server, functools
import numpy as np

FS, DEC, OFF = 480_000, 20, 120_000
HERE = os.path.dirname(os.path.abspath(__file__))
LIVE = os.path.join(HERE, "../../target/release/examples/live")
XEON = os.environ.get("FERRITE_XEON", "xeon")

PAGE = """<!doctype html><meta charset=utf-8><title>SSTV live</title>
<style>body{background:#111;color:#ddd;font:15px system-ui;margin:16px}img{width:640px;max-width:100%;image-rendering:pixelated;border:1px solid #333}</style>
<p id=s>waiting</p><img id=i src="pic.bmp">
<script>setInterval(()=>{let t=Date.now();document.getElementById('i').src='pic.bmp?'+t;
fetch('pic.bmp.txt?'+t).then(r=>r.text()).then(x=>document.getElementById('s').textContent=x)},1000)</script>"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mhz", type=float)
    ap.add_argument("seconds", type=float)
    ap.add_argument("out")
    ap.add_argument("--vis", type=int)
    ap.add_argument("--gain", type=int, default=50)
    ap.add_argument("--serve", type=int)
    a = ap.parse_args()
    d = os.path.dirname(os.path.abspath(a.out))
    if a.serve:
        with open(os.path.join(d, "index.html"), "w") as f:
            f.write(PAGE)
        h = functools.partial(http.server.SimpleHTTPRequestHandler, directory=d)
        http.server.ThreadingHTTPServer.allow_reuse_address = True
        srv = http.server.ThreadingHTTPServer(("127.0.0.1", a.serve), h)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        print(f"watch: http://127.0.0.1:{a.serve}/", flush=True)
    n = int(a.seconds * FS)
    f = int(round(a.mhz * 1e6)) + OFF
    # into a plain file there (never blocks: a FIFO's reader falling behind made bladeRF-cli drop samples, and the
    # tones read 1.4x high, Nov 15), followed by tail; the reader here stops after n samples
    # an earlier run's capture still going holds the device: stopped first (by its exact name)
    remote = (f"pkill -x bladeRF-cli; sleep 0.5; rm -f /tmp/sstvcap.sc16; touch /tmp/sstvcap.sc16; (bladeRF-cli -e 'set frequency rx {f}; set samplerate rx {FS}; "
              f"set bandwidth rx 1500000; set gain rx {a.gain}; rx config file=/tmp/sstvcap.sc16 format=bin n={n}; rx start; rx wait' "
              f">/dev/null 2>&1 &); exec tail -c +1 -f /tmp/sstvcap.sc16")
    # one shared connection: the Xeon's sshd turns away bursts of new ones (Nov 15)
    cm = ["-o", "ControlMaster=auto", "-o", f"ControlPath={os.path.expanduser('~')}/.ssh/cm-sstv-%C", "-o", "ControlPersist=900"]
    src = subprocess.Popen(["ssh", *cm, XEON, remote], stdout=subprocess.PIPE)
    live = subprocess.Popen([LIVE, "24000", a.out] + ([str(a.vis)] if a.vis else []), stdin=subprocess.PIPE)
    chunk = FS // 10 * 4  # 0.1 s of sc16 IQ
    left = n * 4
    ph, last = 0.0, 1 + 0j
    w = 2 * np.pi * OFF / FS
    try:
        while True:
            b = src.stdout.read(min(chunk, left))
            if not b:
                break
            left -= len(b)
            iq = np.frombuffer(b[: len(b) // (4 * DEC) * 4 * DEC], np.int16).astype(np.float32).reshape(-1, 2)
            z = iq[:, 1] + 1j * iq[:, 0]  # the bladeRF's mirror: Q first (as bladecap.sh)
            k = np.arange(len(z))
            z = z * np.exp(-1j * (ph + w * k))  # the signal sits OFF above the centre after the swap: down to 0
            ph = (ph + w * len(z)) % (2 * np.pi)
            y = z.reshape(-1, DEC).mean(axis=1)
            yy = np.concatenate([[last], y])
            last = y[-1]
            hz = np.angle(yy[1:] * np.conj(yy[:-1])) * (FS / DEC) / (2 * np.pi)
            if os.environ.get("SSTV_DEBUG"):
                print(f"read {len(b)} B, audio rms {np.std(hz):.0f} Hz, mean {np.mean(hz):.0f} Hz", file=sys.stderr, flush=True)
            pcm = np.clip(hz * 6, -32767, 32767).astype("<i2").tobytes()
            if os.environ.get("SSTV_DUMP"):
                with open(os.environ["SSTV_DUMP"], "ab") as f:
                    f.write(pcm)
            live.stdin.write(pcm)
            if left <= 0:
                break
            live.stdin.flush()
    except (KeyboardInterrupt, BrokenPipeError):
        pass
    finally:
        src.terminate()
        live.stdin.close()
        live.wait()


if __name__ == "__main__":
    main()
