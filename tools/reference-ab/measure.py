"""Set bands from a NeoFilter preset JSON on the reference EQ, render an impulse."""
import sys, os, json, math
import numpy as np
from scipy.io import wavfile
from pedalboard import load_plugin

SHAPES = ["bell","low_shelf","low_cut","high_shelf","high_cut","notch","band_pass","tilt_shelf","flat_tilt"]
SLOPES = [6,12,18,24,30,36,48,72,96]

def setup(p, bands, verbose=True):
    for n in range(1, 25):
        p.parameters[f"band_{n}_used"].raw_value = 0.0
    for b in bands:
        n = b["band"]
        P = lambda k: p.parameters[f"band_{n}_{k}"]
        P("used").raw_value = 1.0
        P("enabled").raw_value = 1.0 if b.get("enabled", True) else 0.0
        P("frequency").raw_value = math.log(b["freq_hz"]/10.0)/math.log(3000.0)
        P("gain").raw_value = (b["gain_db"]+30.0)/60.0
        P("q").raw_value = math.log(b["q"]/0.025)/math.log(1600.0)
        P("shape").raw_value = SHAPES.index(b["shape"])/8.0
        P("slope").raw_value = SLOPES.index(b["slope"])/9.0
        P("stereo_placement").raw_value = 0.5
        if verbose:
            def val(k):
                s = str(P(k)); i = s.find("value="); return s[i+6:].split(" range")[0].split(" (")[0].split(" boolean")[0].rstrip(">")
            print(n, "|", val("used"), "|", val("enabled"), "|", val("shape"), "|", val("frequency"), "|", val("gain"), "|", val("q"), "|", val("slope"), "|", val("stereo_placement"))

def impulse(p, sr, length=131072):
    silence = np.zeros((2, sr), dtype=np.float32)
    p(silence, sr, reset=True)
    p(silence, sr, reset=False)
    x = np.zeros((2, length), dtype=np.float32)
    x[:, 0] = 1.0
    return p(x, sr, reset=False)

if __name__ == "__main__":
    preset, sr, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    p = load_plugin(os.path.expanduser("~/.vst3/yabridge/FabFilter Pro-Q 3.vst3"))
    for k in ["processing_mode","processing_resolution","gain_scale","output_level","auto_gain","bypass"]:
        print(k, str(p.parameters[k]).split("raw_value=")[1][:60])
    setup(p, json.load(open(preset))["bands"])
    y = impulse(p, sr)
    print("peak", float(np.abs(y).max()), "argmax", int(np.abs(y[0]).argmax()), "L==R", bool(np.allclose(y[0], y[1])), "tail", float(np.abs(y[0][-1000:]).max()))
    wavfile.write(out, sr, y[0].astype(np.float32))
