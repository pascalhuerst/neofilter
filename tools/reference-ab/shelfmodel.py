import sys, json; sys.path.insert(0, "/tmp/abtest/model")
import numpy as np, refmodel as rm
SQ2 = np.sqrt(2)
def shelf_sections(kind, f0, g, q, slope):
    """analog sections of the shelf prototype with the measured resonance law; returns (list of (bz,cz,bp,cp)), first-order list [(z, p)], gain k)"""
    N = slope//6; G = 10**(g/20); w0 = 2*np.pi*f0; r = G**(1/(2*N)); qeff = q**0.47
    wz, wp = (w0/r, w0*r) if kind == "high_shelf" else (w0*r, w0/r)
    k = G if kind == "high_shelf" else 1.0
    secs = []; first = []
    for i in range(N//2):
        Q = 1/(2*np.sin(np.pi*(2*i+1)/(2*N)))
        if i == 0: Q *= qeff
        secs.append((wz/Q, wz*wz, wp/Q, wp*wp))
    if N % 2: first.append((wz, wp))
    return secs, first, k
def shelf_db(kind, f0, g, q, slope, f, fs, rule=True):
    secs, first, k = shelf_sections(kind, f0, g, q, slope); tot = np.full(len(f), 20*np.log10(k)); nyq = fs/2
    for sec in secs:
        r = None
        if rule and slope == 12 and abs(q-1) < 1e-6 and 0.5*f0 < 0.999*nyq:
            d = 1/SQ2; c = 0.5*f0
            r = rm.interpolate(rm.sec_magsq(sec), [(0.0, 0.0), (c*(1-d),)*2, (c*(1-0.05*d),)*2, (c,)*2, (nyq, nyq)], fs)
        tot += rm.response_db([r], f, fs) if r is not None else 10*np.log10(rm.sec_magsq(sec)(f))
    for wz, wp in first:
        w = 2*np.pi*f; tot += 10*np.log10((w*w+wz*wz)/(w*w+wp*wp))
    return tot
if __name__ == "__main__":
    d = json.load(open("/tmp/abtest/reference_shelves.json"))
    for c in d["cases"]:
        f = np.array(c["freqs_hz"]); ref = np.array(c["db"]); fs = c["sample_rate"]; sh = c["shape"]
        if sh == "tilt_shelf": m = shelf_db("high_shelf", c["freq_hz"], 2*c["gain_db"], c["q"], c["slope"], f, fs) - c["gain_db"]
        else: m = shelf_db(sh, c["freq_hz"], c["gain_db"], c["q"], c["slope"], f, fs)
        e = np.max(abs(m-ref)); lim = f <= 0.1*fs; e_lo = np.max(abs(m-ref)[lim])
        print(f"{e:8.5f} dB (below 0.1 fs: {e_lo:.5f})  {fs} {sh} {c['freq_hz']} {c['gain_db']:+} Q{c['q']} s{c['slope']}")
