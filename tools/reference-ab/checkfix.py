import sys, json; sys.path.insert(0, "/tmp/abtest/model")
import numpy as np, refmodel as rm
d = json.load(open("/tmp/abtest/reference_curves.json"))
worst = []
for c in d["cases"]:
    f = np.array(c["freqs_hz"]); ref = np.array(c["db"]); fs = c["sample_rate"]
    s = rm.design_bell(c["freq_hz"], c["gain_db"], c["q"], c["slope"], fs) if c["shape"] == "bell" else rm.design_highcut(c["freq_hz"], c["q"], c["slope"], fs)
    if s is None: worst.append((float("inf"), c)); continue
    m = rm.response_db(s, f, fs); sel = ref > -90
    worst.append((float(np.max(np.abs(m-ref)[sel])), c))
worst.sort(key=lambda t: -t[0])
print("cases", len(worst), "none:", sum(1 for w in worst if w[0] == float("inf")))
for e, c in worst[:22]:
    print(f"{e:9.5f} dB  {c['sample_rate']} {c['shape']} {c['freq_hz']} {c['gain_db']:+} Q{c['q']} s{c['slope']}")
errs = np.array([w[0] for w in worst if np.isfinite(w[0])]); print("median", np.median(errs), "90%", np.quantile(errs, 0.9), "<0.001:", np.mean(errs < 0.001), "<0.01:", np.mean(errs < 0.01))
