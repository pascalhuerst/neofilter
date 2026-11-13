import sys, os, json; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lab import *
cases = []
def add(shape, f, g, q, slope, rates=(44100, 48000, 96000)):
    for fs in rates: cases.append((fs, shape, f, g, q, slope))
# default-Q 12 dB/oct shelves, incl. near the top of the band
for f, g in ((722.9, -1.32), (3000, 6), (8000, -9), (12000, 9), (15000, 6), (16000, -6), (10000, 18)):
    add("high_shelf", f, g, 1, 12)
for f, g in ((200, 4), (5000, -6), (12000, 12), (15000, -9)):
    add("low_shelf", f, g, 1, 12)
# resonance law (low frequency, where the analog prototype is what matters)
for q in (0.5, 0.71, 1.41, 2, 3, 6):
    add("high_shelf", 1000, 6, q, 12, rates=(96000,)); add("low_shelf", 800, -9, q, 12, rates=(96000,))
for q in (0.5, 1, 3):
    add("high_shelf", 1000, 12, q, 24, rates=(96000,)); add("low_shelf", 1000, -8, q, 24, rates=(96000,))
# tilt shelf convention
for sl in (6, 12, 24):
    add("tilt_shelf", 1000, 6, 1, sl, rates=(96000,)); add("tilt_shelf", 2000, -4, 1, sl, rates=(96000,))
add("tilt_shelf", 1000, 6, 3, 12, rates=(96000,))
out = []
for fs, shape, f, g, q, slope in cases:
    h = ref_ir([band(shape, f, g, q, slope)], fs, 131072)
    Hh = np.fft.rfft(h); fb = np.fft.rfftfreq(len(h), 1/fs)
    freqs = np.geomspace(40, 0.47*fs, 72); dbv = np.interp(freqs, fb, 20*np.log10(np.abs(Hh)+1e-300))
    out.append({"sample_rate": fs, "shape": shape, "freq_hz": f, "gain_db": g, "q": q, "slope": slope,
                "freqs_hz": [round(float(x), 3) for x in freqs], "db": [round(float(x), 5) for x in dbv]})
json.dump({"description": "Shelf and tilt-shelf responses measured on the reference EQ (black-box impulse responses, 32-bit float)", "cases": out}, open("/tmp/abtest/reference_shelves.json", "w"))
print(len(out), "cases")
