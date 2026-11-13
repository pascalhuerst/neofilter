"""Reference-matched digital design rules (derived from black-box measurements).

Every 2nd-order analog section is converted to a digital biquad whose squared
magnitude interpolates the analog section's squared magnitude at five
frequencies: DC, three interior points, and Nyquist (which takes the analog
value at a frequency f_x <= Nyquist).
"""
import numpy as np

SQRT2 = np.sqrt(2.0)

# ---------- generic 5-point magnitude interpolation ----------
def phi(f, fs):
    p1 = np.sin(np.pi*f/fs)**2; p0 = 1-p1
    return np.array([p0, p1, 4*p0*p1])

def _factor(X):
    X0, X1, X2 = X
    if X0 < 0 or X1 < 0: return None
    W = 0.5*(np.sqrt(X0)+np.sqrt(X1)); disc = W*W + X2
    if disc < 0: return None
    c0 = 0.5*(W+np.sqrt(disc))
    if c0 <= 0: return None
    return np.array([c0, 0.5*(np.sqrt(X0)-np.sqrt(X1)), -X2/(4*c0)])

def interpolate(magsq, pts, fs):
    """pts: list of (digital_freq, analog_freq). Returns (b, a) with a[0]=1, or None if not realisable.

    Solved in the basis (1, t, t^2), t = u/u_s, u = sin^2(pi f/fs), with u_s the largest
    interior u and every row scaled, so that low frequencies stay well conditioned."""
    us = [np.sin(np.pi*fd/fs)**2 for fd, _ in pts]
    inner = [u for u in us if 0 < u < 0.999999]
    u_s = max(inner) if inner else 1.0
    rows = []
    for (fd, fa), u in zip(pts, us):
        v = magsq(fa); t = u/u_s; r = np.array([1.0, t, t*t, -v, -v*t, -v*t*t])
        rows.append(r/np.max(np.abs(r)))
    _, _, vt = np.linalg.svd(np.array(rows)); x = vt[-1]
    if x[3] < 0: x = -x
    def to_X(c):
        c0, c1, c2 = c[0], c[1]/u_s, c[2]/(u_s*u_s)       # polynomial in u
        return np.array([c0, c0 + c1 + c2, -c2/4])
    b = _factor(to_X(x[:3])); a = _factor(to_X(x[3:]))
    if b is None or a is None or not np.all(np.isfinite(b)) or not np.all(np.isfinite(a)): return None
    if not (abs(a[2]/a[0]) < 1 and abs(a[1]/a[0]) < 1 + a[2]/a[0]): return None
    return b/a[0], a/a[0]

def interpolate_with_nyquist_fallback(magsq, interior, fs, fx):
    """Use Nyquist target f_x; if not realisable, lower f_x to the largest realisable value."""
    nyq = fs/2
    mk = lambda fxx: interpolate(magsq, [(0.0, 0.0)] + [(f, f) for f in interior] + [(nyq, fxx)], fs)
    r = mk(fx)
    if r is not None: return r
    lo = max(interior)*1.0001; hi = fx
    if mk(lo) is None:
        # scan for any realisable value
        for t in np.linspace(hi, lo, 200):
            if mk(t) is not None: lo = t; break
        else: return None
    for _ in range(60):
        mid = 0.5*(lo+hi)
        if mk(mid) is not None: lo = mid
        else: hi = mid
    return mk(lo)

# ---------- analog prototypes ----------
# A section is (bz, cz, bp, cp):  (s^2 + bz s + cz) / (s^2 + bp s + cp), s in rad/s.
def sec_magsq(sec):
    bz, cz, bp, cp = sec
    def m(f):
        w2 = (2*np.pi*f)**2
        return ((cz-w2)**2 + bz*bz*w2)/((cp-w2)**2 + bp*bp*w2)
    return m

def _quad(r1, r2):
    """monic quadratic with roots r1, r2 (conjugate pair or two reals) -> (b, c)"""
    return (-(r1+r2)).real, (r1*r2).real

def bell_sections(f0, gain_db, q, slope):
    """Sections of the Butterworth-shelf band prototype (low-pass shelf of order M through
    s -> Qs (s/w0 + w0/s)), zeros paired with the poles of the same band side, ascending."""
    M = max(1, slope//12); G = 10**(gain_db/20); g = G**(1.0/M); Qs = q/SQRT2; w0 = 2*np.pi*f0
    secs = []
    def band_roots(r):
        # s^2 - (r/Qs) w0 s + w0^2 = 0 for a prototype root r
        return np.roots([1, -(r/Qs)*w0, w0*w0])
    for k in range(M//2):
        d = 2*np.sin(np.pi*(2*k+1)/(2*M))
        rz = np.roots([1, d*np.sqrt(g), g]); rp = np.roots([1, d/np.sqrt(g), 1/g])
        # each complex prototype root gives one lower-side and one upper-side band root
        zr = sorted(band_roots(rz[0]), key=abs); pr = sorted(band_roots(rp[0]), key=abs)
        for side in (0, 1):
            bz, cz = _quad(zr[side], np.conj(zr[side])); bp, cp = _quad(pr[side], np.conj(pr[side]))
            secs.append((bz, cz, bp, cp))
    if M % 2:
        zr = band_roots(-np.sqrt(g)); pr = band_roots(-1/np.sqrt(g))
        bz, cz = _quad(zr[0], zr[1]); bp, cp = _quad(pr[0], pr[1])
        secs.append((bz, cz, bp, cp))
    return sorted(secs, key=lambda s: s[3])

def extrema(sec):
    """positive extremum frequencies (Hz) of the section magnitude, ascending"""
    bz, cz, bp, cp = sec
    n1 = bz*bz - 2*cz; n0 = cz*cz; d1 = bp*bp - 2*cp; d0 = cp*cp
    A, B, C = d1-n1, 2*(d0-n0), n1*d0-n0*d1
    scale = max(abs(A)*cp*cp, abs(B)*cp, abs(C), 1e-300)
    if abs(A)*cp*cp < 1e-12*scale:
        r = np.array([-C/B]) if abs(B)*cp > 1e-12*scale else np.array([])
    else:
        disc = B*B - 4*A*C
        if disc < 0: return []
        r = np.array([(-B - np.sqrt(disc))/(2*A), (-B + np.sqrt(disc))/(2*A)])
    return sorted(np.sqrt(r[r > 0])/2/np.pi)

# ---------- shape rules ----------
def design_bell_section(sec, q, fs):
    nyq = fs/2; ms = sec_magsq(sec); bz, cz, bp, cp = sec
    if abs(cz-cp) <= 1e-9*cp:                         # bell-type section (same zero and pole frequency)
        f0 = np.sqrt(cp)/2/np.pi; d = min(0.8, max(0.1, 1/q))
        interior = [f0*(1-d), f0*(1-0.05*d), f0]; fx = min(nyq, max(0.9*nyq, 1.2*f0))
    else:
        ex = extrema(sec)
        if not ex: return None
        fe = ex[0]
        if len(ex) == 2 and ex[1] < 0.999*nyq:
            # the lower extremum is skipped when it is insignificant (hardly differs from the DC level)
            depth = abs(1 - ms(ex[0])/ms(0.0))
            if depth < 1e-5 or (ex[0] < 0.1*ex[1] and depth < 1e-3): fe = ex[1]
        d = min(0.95, 0.5/q + 0.006)
        interior = [fe*(1-d), fe*0.999, fe]; fx = min(nyq, max(0.83*nyq, 1.8*fe))
    if max(interior) >= 0.999*nyq: return None
    return interpolate_with_nyquist_fallback(ms, interior, fs, fx)

def design_bell(f0, gain_db, q, slope, fs):
    out = []
    for sec in bell_sections(f0, gain_db, q, slope):
        r = design_bell_section(sec, q, fs)
        if r is None: return None
        out.append(r)
    return out

def bell_analog_db(f0, gain_db, q, slope, f):
    tot = np.zeros(len(f))
    for sec in bell_sections(f0, gain_db, q, slope): tot += 10*np.log10(sec_magsq(sec)(np.asarray(f)))
    return tot

def highcut_Qs(order, q):
    Qs = [1/(2*np.sin(np.pi*(2*k+1)/(2*order))) for k in range(order//2)]
    if Qs: Qs[0] *= q
    return Qs

def design_highcut_section(f0, Q, fs):
    nyq = fs/2
    ms = lambda f: abs(1/((1j*f/f0)**2 + (1j*f/f0)/Q + 1))**2
    c = max(0.5, np.sqrt(1-1/(2*Q*Q)) if Q > 1/SQRT2 else 0.0)
    interior = [f0*c/2, f0*c, f0]
    if f0 >= 0.999*nyq: return None
    return interpolate_with_nyquist_fallback(ms, interior, fs, nyq)

def design_highcut(f0, q, slope, fs):
    order = slope//6
    if order % 2: return None            # first-order sections: rule not identified
    out = []
    for Q in highcut_Qs(order, q):
        r = design_highcut_section(f0, Q, fs)
        if r is None: return None
        out.append(r)
    return out

def response_db(sections, f, fs):
    w = 2*np.pi*np.asarray(f)/fs; z = np.exp(-1j*w); tot = np.zeros(len(w))
    for b, a in sections:
        tot += 20*np.log10(np.abs(np.polyval(b[::-1], z)/np.polyval(a[::-1], z)))
    return tot
