import json, numpy as np
from scipy.io import wavfile
out = []
for name in ("friend-eq-a", "friend-eq-b", "friend-eq-ab"):
    for fs in (44100, 48000, 96000):
        _, y = wavfile.read(f"/tmp/abtest/full/{name}-{fs}.wav")
        Hh = np.fft.rfft(y.astype(np.float64)); fb = np.fft.rfftfreq(len(y), 1/fs)
        freqs = np.geomspace(40, 20000, 160); dbv = np.interp(freqs, fb, 20*np.log10(np.abs(Hh)))
        out.append({"preset": name, "sample_rate": fs, "freqs_hz": [round(float(x), 3) for x in freqs], "db": [round(float(x), 5) for x in dbv]})
json.dump({"description": "Total magnitude responses of the factory presets measured on the reference EQ (black-box impulse responses)", "cases": out}, open("/tmp/abtest/reference_presets.json", "w"))
