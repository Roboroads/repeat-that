# Test fixtures

The macOS end-to-end test (`../macos_audio.rs`) plays these with `afplay`. Both are synthetic, made with Python's
standard library, so they have no author and no license to follow. Both are 8 kHz, mono, 16-bit.

- `tone_1khz.wav`: a 1 kHz sine at half of full scale, 20 seconds. Its level is easy to check.
- `noise.wav`: noise at an RMS of 0.2 (a sum of four uniform random numbers, seed 1), 15 seconds. A copy of noise that
  arrives a little later adds to it instead of cancelling it, as a copy of a tone can. That makes it the signal for the
  click check, where the app and our replay of it play together for a moment.

```sh
python3 -c "
import math, random, struct, wave
def write(name, seconds, sample):
    w = wave.open(name, 'wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
    w.writeframes(b''.join(struct.pack('<h', round(32767 * sample(n))) for n in range(8000 * seconds)))
    w.close()
write('tone_1khz.wav', 20, lambda n: 0.5 * math.sin(2 * math.pi * 1000 * n / 8000) * 16384 / 16383.5)
rng = random.Random(1)
write('noise.wav', 15, lambda n: max(-0.9, min(0.9, sum(rng.uniform(-1, 1) for _ in range(4)) * 0.2 / (4 / 3) ** 0.5)))
"
```

Only add synthetic audio or recordings with a license that allows redistribution.
