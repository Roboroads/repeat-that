# Test fixtures

- `tone_1khz.wav`: a synthetic 1 kHz sine at half of full scale, 20 seconds, 8 kHz, mono, 16-bit. The macOS
  end-to-end test (`../macos_audio.rs`) plays it with `afplay`. Made with Python's standard library, so it has no
  author and no license to follow:

  ```sh
  python3 -c "
  import math, struct, wave
  w = wave.open('tone_1khz.wav', 'wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
  w.writeframes(b''.join(struct.pack('<h', round(16384 * math.sin(2 * math.pi * 1000 * n / 8000)))
                         for n in range(8000 * 20)))
  w.close()"
  ```

Only add synthetic audio or recordings with a license that allows redistribution.
