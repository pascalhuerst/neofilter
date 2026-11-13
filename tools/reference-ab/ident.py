"""Identify the digital filter (poles/zeros) of a measured impulse response, and
compare with an analog prototype's poles/zeros."""
import sys, numpy as np
from scipy.io import wavfile

def prony(h, order, nfit=None):
    h = np.asarray(h, dtype=np.float64)
    if nfit is None:
        # use the part of the IR that is above the float32 noise floor
        env = np.abs(h); thr = env.max()*1e-5
        nz = np.nonzero(env > thr)[0]; nfit = max(int(nz[-1])+1, 8*order)
    N = order
    rows = np.array([h[n-N:n][::-1] for n in range(N+1, nfit)])
    rhs = -h[N+1:nfit]
    a = np.linalg.lstsq(rows, rhs, rcond=None)[0]
    a = np.concatenate([[1.0], a])
    b = np.convolve(a, h[:N+1])[:N+1]
    res = np.sqrt(np.mean((rows@a[1:] - rhs)**2))/np.sqrt(np.mean(h[:nfit]**2))
    return b, a, res

def butter_dampings(M):
    """(list of 2cos(theta) for pairs, has_real_pole)"""
    pairs = []
    for k in range(M//2):
        theta = np.pi*(2*k+1)/(2*M)
        pairs.append(2*np.sin(theta))  # damping coefficient d: s^2 + d s + 1
    return pairs, (M % 2 == 1)

def bell_analog_pz(f0, gain_db, q_disp, slope, qfac=1/np.sqrt(2)):
    """Poles and zeros (rad/s) of the NeoFilter analog bell prototype."""
    M = max(1, slope//12); G = 10**(gain_db/20); g = G**(1.0/M)
    Qs = q_disp*qfac; w0 = 2*np.pi*f0
    pairs, real = butter_dampings(M)
    pz, pp = [], []   # prototype zeros / poles (normalised lowpass shelf)
    for d in pairs:
        pz += list(np.roots([1, d*np.sqrt(g), g])); pp += list(np.roots([1, d/np.sqrt(g), 1/g]))
    if real:
        pz.append(-np.sqrt(g)); pp.append(-1/np.sqrt(g))
    def bp(r):  # s_lp = Qs (s/w0 + w0/s)  ->  s^2 - (r/Qs) w0 s + w0^2 = 0
        return list(np.roots([1, -(r/Qs)*w0, w0*w0]))
    Z = [x for r in pz for x in bp(r)]; P = [x for r in pp for x in bp(r)]
    return np.array(Z), np.array(P)

def fmt(roots, fs):
    out = []
    for r in sorted(roots, key=lambda z: (abs(np.angle(z)), abs(z))):
        if r.imag < -1e-12: continue
        s = np.log(r)*fs
        out.append(f"|z|={abs(r):.6f} ang={np.angle(r):.6f}  ~ f={abs(s)/2/np.pi:9.2f} Hz Q={abs(s)/(-2*s.real) if s.real<0 else float('inf'):8.3f}")
    return out

if __name__ == "__main__":
    wav, order = sys.argv[1], int(sys.argv[2])
    fs, y = wavfile.read(wav)
    b, a, res = prony(y, order)
    print("residual", res, "b0", b[0])
    print("ref poles:"); print("\n".join("  "+l for l in fmt(np.roots(a), fs)))
    print("ref zeros:"); print("\n".join("  "+l for l in fmt(np.roots(b), fs)))
    if len(sys.argv) > 3:
        f0, g, q, slope = float(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5]), int(sys.argv[6])
        Z, P = bell_analog_pz(f0, g, q, slope)
        print("analog proto poles (as matched-z):"); print("\n".join("  "+l for l in fmt(np.exp(P/fs), fs)))
        print("analog proto zeros (as matched-z):"); print("\n".join("  "+l for l in fmt(np.exp(Z/fs), fs)))
