import sys, os, json, glob
import numpy as np
from scipy.io import wavfile
from pedalboard import load_plugin
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from measure import setup, impulse
d = sys.argv[1]; rates = [int(r) for r in sys.argv[2].split(",")]
p = load_plugin(os.path.expanduser("~/.vst3/yabridge/FabFilter Pro-Q 3.vst3"))
for f in sorted(glob.glob(os.path.join(d, "*.json"))):
    setup(p, json.load(open(f))["bands"], verbose=False)
    for sr in rates:
        y = impulse(p, sr)
        wavfile.write(f[:-5] + f"-{sr}.wav", sr, y[0].astype(np.float32))
print("done")
