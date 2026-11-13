import os, sys, math, numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from measure import setup, impulse
from ident import prony, bell_analog_pz, fmt
from pedalboard import load_plugin
_p = None
def plugin():
    global _p
    if _p is None:
        _p = load_plugin(os.path.expanduser("~/.vst3/yabridge/FabFilter Pro-Q 3.vst3"))
    return _p
def band(shape, f, g, q, slope, n=1, enabled=True):
    return {"band": n, "enabled": enabled, "shape": shape, "freq_hz": f, "gain_db": g, "q": q, "slope": slope}
def ref_ir(bands, sr, length=65536):
    import hashlib, json
    key = hashlib.sha1(json.dumps([bands, sr, length], sort_keys=True).encode()).hexdigest()[:20]
    path = f"/tmp/abtest/cache/{key}.npy"
    if os.path.exists(path):
        return np.load(path)
    p = plugin(); setup(p, bands, verbose=False)
    y = impulse(p, sr, length)[0].astype(np.float64)
    os.makedirs("/tmp/abtest/cache", exist_ok=True); np.save(path, y)
    return y
def H(b, a, w):
    z = np.exp(-1j*np.asarray(w)); return np.polyval(b[::-1], z)/np.polyval(a[::-1], z)
def bell_analog(f0, g, q, f):
    """12 dB/oct analog bell magnitude at frequency f (Hz)"""
    G = 10**(g/20); Qs = q/np.sqrt(2); s = 1j*np.asarray(f)/f0
    return abs((s*s + s*np.sqrt(G)/Qs + 1)/(s*s + s/(np.sqrt(G)*Qs) + 1))
def sq(z, fs):
    """digital root -> (freq Hz, Q) of the equivalent s-plane root"""
    s = np.log(complex(z))*fs
    return abs(s)/2/np.pi, (abs(s)/(-2*s.real) if s.real < 0 else float("inf"))
def upper(roots):
    return sorted([r for r in roots if r.imag >= -1e-12], key=lambda z: abs(np.angle(z)))
def shelf_analog(kind, f0, g, f, damp=np.sqrt(2)):
    """12 dB/oct symmetric shelf (NeoFilter prototype, display Q 1 -> damp sqrt2)"""
    G = 10**(g/20); s = 1j*np.asarray(f, dtype=float)/f0
    if kind == "low_shelf":
        gg = np.sqrt(G); return abs((s*s + damp*np.sqrt(gg)*s + gg)/(s*s + damp*s/np.sqrt(gg) + 1/gg))
    gg = np.sqrt(1/G); return G*abs((s*s + damp*np.sqrt(gg)*s + gg)/(s*s + damp*s/np.sqrt(gg) + 1/gg))
def cut_analog(kind, f0, q, f):
    s = 1j*np.asarray(f, dtype=float)/f0; Qs = q/np.sqrt(2); d = s*s + s/Qs + 1
    return abs(s*s/d) if kind == "low_cut" else abs(1/d)
def crossings(b, a, fs, target, lo=200):
    f = np.linspace(1, fs/2-1, 400001); w = 2*np.pi*f/fs
    d = 20*np.log10(abs(H(b,a,w))) - 20*np.log10(target(f))
    c = f[np.nonzero(np.diff(np.sign(d)))[0]]
    return c[c > lo], d
