import sys, os
from pedalboard import load_plugin
p = load_plugin(os.path.expanduser("~/.vst3/yabridge/FabFilter Pro-Q 3.vst3"))
print(p.name, "latency?", getattr(p, "latency_samples", None))
names = list(p.parameters.keys())
print(len(names))
for n in names[:16]:
    par = p.parameters[n]
    print(n, "| raw", round(par.raw_value, 5), "| str", str(par), "| range", getattr(par, "range", None), "| units", getattr(par, 'units', None))
print([n for n in names if not n.startswith("band_")])
