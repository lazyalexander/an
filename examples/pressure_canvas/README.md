# Pressure probe

The gate for replacing Krita with An's own sketch canvas: does a web page receive genuine pen pressure, tilt and a usable sample rate?

```sh
python3 examples/pressure_canvas/serve.py
```

Open the printed URL on:

1. **iPad Safari with Apple Pencil**, same Wi-Fi as the Mac (no injector needed);
2. **Mac Chrome or Safari** driven through the Codonic Pad injector.

Draw strokes from very light to very hard, tilt the pencil, then press **上传记录**. Each report lands in `reports/` with the raw samples and a summary: pointer types, pressure range, distinct pressure values, tilt, median sample rate and event latency.

Pass: `types` includes `pen`, pressure spans most of 0–1 with many distinct values, tilt is seen, sample rate ≥ 120 Hz on iPad.

The brush borrows Krita's ideas in miniature: a pressure curve, spacing-based dabs, weighted stroke smoothing and every coalesced hardware sample.
