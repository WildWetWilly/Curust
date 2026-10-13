import numpy as np
a = np.empty((0, 2))
print(a.shape, len(a))
seq = np.empty((0, 2))
pts = [(0.0, 0.0), (10.0, 0.1), (10.3, 10.0), (0.2, 10.0), (0.0, 0.05), (5, 5)]
for x, y in pts:
    seq = np.concatenate([seq, np.array([[x, y]])])
print(seq.shape, seq[1][0], seq[-1][1], seq[2])
b = np.copy(seq)
b[1] = b[1] + np.array([0.5, 0.5])
print(b[1], seq[1])
d = ((seq[0] - seq[1:]) ** 2).sum(1)
print(d, np.amax(d), np.argmax(d >= 50.0), np.argmax(d >= 1e9))
r = np.roll(seq, -2, 0)
print(r[0], np.resize(r, (4, 2))[3], np.resize(np.roll(seq, -1, 0)[::-1], (3, 2)))
mrot = np.array([[0, -1], [1, 0]])
xp = np.dot(mrot, seq[2] - seq[1])
print(xp, xp / np.sqrt((xp ** 2).sum(-1)))
vd1 = seq[:-1]
vd2 = seq[1:]
t = seq[3] + xp * 0.4
rel = np.clip(((t - vd1) * (vd2 - vd1)).sum(1) / ((vd2 - vd1) * (vd2 - vd1)).sum(1), 0., 1.)
near = vd1 + rel[:, np.newaxis] * (vd2 - vd1)
dist = ((t - near) * (t - near)).sum(1)
print(rel, near[2], dist, np.amin(dist))
print(np.floor_divide(len(seq), 3), np.fabs(-2.5), np.sqrt(2.0), seq[3:0:-1][0], seq[2::-1][2])
lb = ((seq[2] - seq[0]) ** 2).sum(0)
rp = ((seq[1] - seq[0]) * (seq[2] - seq[0])).sum(0)
print(lb, rp, rp / lb, "{:.3f}".format(seq[1][0] - 0.0004), str(seq[2][1] * 3))
z = np.array([0.1] * 20)
print(z.sum(), np.sum(z), sum([0.1] * 20), np.array([[1, 2], [3, 4]]).sum(0), np.array([1, 2, 3]).dtype)
print(np.array([0.0, float("nan"), 1.0]).max(), np.amin(np.array([3.0, 1.0, 2.0])), np.argmax([1, 3, 3]))
