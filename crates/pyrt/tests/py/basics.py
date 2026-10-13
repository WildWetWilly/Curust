# Core language
x = [1, 2, 3]
print(x, len(x), x[-1], x[::-1], x[1:])
d = {"a": 1, "b": 2}
d["c"] = 3
print(d, list(d.keys()), sorted(d.items(), key=lambda kv: -kv[1]))
print({k: v * 2 for k, v in d.items() if v > 1})
print([i * i for i in range(10) if i % 2], sum(i for i in range(5)))
s = "G1 X10.5 Y20 ;comment"
print(s.split(), s.split(" ", 1), s.find("X"), s[s.find("X") + 1:], s.rsplit(" ", 1))
print(s.upper(), s.lower(), s.replace("X", "x"), s.strip(";"), "  a b  ".strip(), s.startswith(("G0", "G1")))
print("%d %5.2f %s %r %-5s| %05d %x %e %g" % (42, 3.14159, "hi", "hi", "ab", 42, 255, 12345.678, 0.0001234))
print("{} {:.3f} {:>8} {:<6}| {:^7} {:,} {:08.3f} {:+d}".format(1, 2.0, "r", "l", "c", 1234567, -3.14159, 5))
print(f"{x[0]} {d['a']!r} {3.0:.2f} {'nested'} {1/3} {2**100 if False else 7}")
a = 10.0
print(f"{a=}", f"{a:g}", f"{0.1+0.2}", 1e16, 1e-5, 123456789.0, 0.1, -0.0, 1.5e300 * 1e10, float("nan"))
print(repr(1.1), str(2.5), 7 / 2, 7 // 2, -7 // 2, 7 % -3, -7.5 // 2, 2 ** -1, round(2.5), round(3.5), round(2.675, 2), round(-0.5))
print(int("42"), int(" -7 "), float("1e3"), int(3.99), int(-3.99), abs(-4), max(3, 7, 2), min([4, 1, 9]), divmod(17, 5))
print(bool(""), bool([0]), not None, 3 in [1, 2, 3], "a" in "cat", 4 not in {1: 2})
t = (1, "a", None)
p, q, r = t
print(p, q, r, t.index("a"), t * 2)
first, *rest = [1, 2, 3, 4]
print(first, rest)
print(sorted(["b", "A", "c"]), sorted([3, 1, 2], reverse=True), list(reversed([1, 2, 3])), list(zip([1, 2], "ab")), list(enumerate("xy", 1)))
print(isinstance(1, int), isinstance(True, int), isinstance(1.0, (int, float)), type(1) == int, type("s").__name__)

class Base:
    count = 0
    def __init__(self, name):
        self.name = name
        Base.count += 1
    def greet(self):
        return "Hello " + self.name
    def __repr__(self):
        return "Base(%r)" % self.name

class Child(Base):
    def __init__(self, name, n):
        super().__init__(name)
        self.n = n
    def greet(self):
        return super().greet() + "!" * self.n
    @staticmethod
    def st(v):
        return v * 2
    @classmethod
    def cm(cls):
        return cls.__name__
    @property
    def double(self):
        return self.n * 2

c = Child("Bob", 3)
print(c.greet(), Base.count, Child.st(4), Child.cm(), c.double, repr(c), [c], str(c))
try:
    {}["missing"]
except KeyError as e:
    print("KeyError", e)
try:
    int("x")
except (ValueError, TypeError) as e:
    print("caught", type(e).__name__, e)
try:
    raise ValueError("boom")
except Exception as e:
    print(repr(e), e.args)
finally:
    print("finally")

def gen_default(a, b=2, *args, c=5, **kw):
    return (a, b, args, c, kw)
print(gen_default(1), gen_default(1, 3, 4, 5, c=9, z=1))

def outer():
    n = 0
    def inner():
        nonlocal n
        n += 1
        return n
    inner(); inner()
    return n
print(outer())

match "height":
    case "layer_no":
        print("layer")
    case "height" | "z":
        print("matched height")
    case _:
        print("other")

i = 0
while True:
    i += 1
    if i > 5:
        break
else:
    print("never")
print(i)
for k in range(3):
    pass
else:
    print("for-else", k)
print("a,b,,c".split(","), "x".join(["1", "2"]), "abc".center(9, "*"), "5".zfill(3), "-5".zfill(4), "Ab Cd".swapcase(), "hello world".title())
print(str(1e22), str(1.0e-7), 1.23456789012345678, 100.0, 2.0 ** 0.5, 10 ** 20, 0.1 * 3)
print("%.0f %.0f %.0f %.1f %.2f" % (0.5, 1.5, 2.5, 0.25, 1.005), "{:.0f}".format(2.5), format(1234.5, ".2"), format(1.0, ".3"), format(0.000012345, ".3g"), format(123456789, ".3e"))
print("{0}{1}{0}".format("a", "b"), "{name}={val:.1f}".format(name="x", val=2), "{:%}".format(0.25), "{:x} {:o} {:b} {:#x}".format(255, 8, 5, 255))
print(" ".join(["%s" % v for v in [1.0, 2, "x", None, True]]), str(True), [1.0, None, "s"])
print(sorted({3, 1, 2}), {1, 2} | {3}, {1, 2} & {2}, len({}), dict(a=1), list("ab"), tuple([1]))
lst = [5, 3, 8]
lst.sort()
lst.append(1)
lst.insert(0, 9)
print(lst, lst.pop(), lst.pop(0), lst, lst.index(5), lst.count(3))
print(hex(255), bin(5), oct(8), ord("A"), chr(66), "%c" % 67, "abc"[1], "abc"[-1:], "%%")
print(any([0, 1]), all([1, 0]), list(map(str, [1, 2])), list(filter(None, [0, 1, 2])), sum([0.1] * 10))
print("line1\nline2\r\nline3".splitlines(), "a\tb".expandtabs(4), "x=1".partition("="), "%.3f" % 1e-10)
