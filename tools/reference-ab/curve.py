import sys, subprocess, io
import numpy as np
from scipy.io import wavfile
name, sr, freqs = sys.argv[1], int(sys.argv[2]), [float(x) for x in sys.argv[3].split(",")]
cli = "/home/paso/development/neofilter/target/release/neofilter-cli"
_, y = wavfile.read(f"/tmp/abtest/probe/{name}-{sr}.wav")
H = np.fft.rfft(y.astype(np.float64)); fb = np.fft.rfftfreq(len(y), 1/sr)
def model(extra):
    out = subprocess.run([cli, "curve", "--preset-file", f"/tmp/abtest/probe/{name}.json", "--sample-rate", str(sr), "--points", "4000"] + extra, capture_output=True, text=True).stdout
    a = np.loadtxt(io.StringIO(out), delimiter=",", skiprows=1); return a[:,0], a[:,1]
fd, dd = model([]); fa, da = model(["--analog"])
print("   freq     ref   digital  analog")
for f in freqs:
    r = 20*np.log10(abs(H[int(round(f/sr*len(y)))])+1e-30)
    print(f"{f:8.0f} {r:8.3f} {np.interp(np.log(f), np.log(fd), dd):8.3f} {np.interp(np.log(f), np.log(fa), da):8.3f}")
