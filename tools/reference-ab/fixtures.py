import sys, os, json; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lab import *
cases = []
def add(shape, f, g, q, slope, rates=(44100, 48000, 96000)):
    for fs in rates:
        if f < 0.42*fs: cases.append((fs, shape, f, g, q, slope))
# bells
for slope in (12, 24, 36, 48, 72, 96):
    add("bell", 1000, 6, 1, slope); add("bell", 4000, -9, 2.5, slope); add("bell", 10000, 6, 3, slope)
    add("bell", 15484, 15.69, 7.23, slope); add("bell", 12000, -12, 8, slope)
for f, g, q, s in ((41.1,2.58,1,12),(98,-1.83,2.73,24),(9729,2.46,3.24,48),(233,-2.5,1,12),(177.7,-5.38,0.68,12),(2093,-2.99,1.25,48),(5074,4.9,10.91,24),(392.7,-2.87,2.97,12),(2035,2.78,5,48),(11787,2.45,4.59,48),
                   (4317,10.58,1.05,24),(701.3,-7.36,2.01,12),(15959,8.51,14.14,48),(13195,-4.62,10,36),(6282,-3.54,10,36),(10011,2.64,5.95,12),(165.7,2.06,2.97,48),
                   (8000,18,1,24),(6000,6,20,24),(17000,10,10,24),(3000,9,0.6,24),(18000,-6,4,12),(6000,12,5,48),(8000,-3,3,48),(10000,3,3.3,48)):
    add("bell", f, g, q, s)
# high cuts (even orders)
for f, q, s in ((13607,1,48),(10000,1,12),(10000,3,12),(8000,2,48),(10000,1,24),(15000,0.8,24),(5000,1,96),(12000,1.5,72),(18000,1,24),(16000,1,36),(3000,1,12)):
    add("high_cut", f, 0, q, s)
out = []
for fs, shape, f, g, q, slope in cases:
    h = ref_ir([band(shape, f, g, q, slope)], fs)
    Hh = np.fft.rfft(h); fb = np.fft.rfftfreq(len(h), 1/fs)
    freqs = np.geomspace(60, 0.47*fs, 72); dbv = np.interp(freqs, fb, 20*np.log10(np.abs(Hh)+1e-300))
    out.append({"sample_rate": fs, "shape": shape, "freq_hz": f, "gain_db": g, "q": q, "slope": slope,
                "freqs_hz": [round(float(x), 3) for x in freqs], "db": [round(float(x), 5) for x in dbv]})
json.dump({"description": "Magnitude responses of single bands measured on the reference EQ (black-box impulse responses, 32-bit float)", "cases": out}, open("/tmp/abtest/reference_curves.json", "w"))
print(len(out), "cases")
