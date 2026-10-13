import re
tests = [
    (r'^-?[0-9]+\.?[0-9]*', ["-12.5 X", "abc", "7", "12."]),
    (r"(^|\s)X[\d\.]+(\s|$)", ["G1 X10.5 Y2", "G1 Y2 X3"]),
    (r";.*$", ["G1 X1 ;hello", "nothing"]),
    (r"\[CAZO:(.*?):CAZO\]", [";[CAZO:G1 X1:CAZO] rest [CAZO:b:CAZO]"]),
    (r";\[CAZD:DELETE:[\s\S]+?:CAZD\](\n|$)", ["a\n;[CAZD:DELETE:x\ny:CAZD]\nb;[CAZD:DELETE:z:CAZD]"]),
    (r"S(\d+|\d.+)", ["M104 S200 T0", "M109 S2.5"]),
    (r"G1 F(\d+\.\d+|\d+) E(-?\d+\.\d+|-?\d+)", ["G1 F2700 E-5", "G1 F1500.5 E2.1"]),
    (r"(?<!\.|\w|\d)0+(\d+)", ["test02 * 020", "007 x"]),
    (r"T(\d*)", ["T1", "M104 T", "XT"]),
    (r"(?P<word>\w+)\s(?P=word)", ["hello hello world"]),
    (r"(?i)m10[46]", ["m104", "M106", "x"]),
    (r"a{2,3}?b|c+?", ["aaab", "ccc"]),
    (r"(?=G1)G\d", ["X G1"]),
    (r"(a)|(b)", ["b"]),
    (r"x*", ["abxd"]),
    (r"\bG0\b", ["G0 G01 G0"]),
    (r"^;LAYER:(\d+)$", [";LAYER:5\nG1\n;LAYER:6"]),
]
for pat, subjects in tests:
    for s in subjects:
        m = re.search(pat, s)
        print(repr(pat), repr(s), m.group(0) if m else None, m.groups() if m else None, m.span() if m else None)
        print("  findall", re.findall(pat, s))
        print("  sub", repr(re.sub(pat, "<\\g<0>>", s)), re.subn(pat, "#", s, count=1))
        print("  split", re.split(pat, s))
print(re.findall(r"^;LAYER:(\d+)$", ";LAYER:5\nG1\n;LAYER:6", re.MULTILINE))
print(re.sub(r"(\d+)", lambda m: str(int(m.group(1)) * 2), "a1 b22 c333"))
print(re.sub(r"(?P<n>\d)", r"[\g<n>\1]", "x1y2"))
print(re.escape("G1 X1.5 (a+b)*[c]"), re.escape("a_b-c"))
p = re.compile("M106(.*)\n")
print(p.sub("", "G1\nM106 S255\nM107\n"), p.pattern, p.search("xx") is None)
m = re.match(r"(\w+) (\w+)?", "hello ")
print(m.groups(), m.groups("dflt"), m.group(1, 2), m.start(2), m.end(1), m[1], m.lastindex)
print(re.fullmatch(r"\d+", "123"), re.fullmatch(r"\d+", "123a"))
print([x.group() for x in re.finditer(r"\d+", "a1b22c")])
print(re.split(r"\s*,\s*", "a , b,c ,d"), re.sub("\\s*,\\s*", "\n", "M117 Hi , G4 S1"))
print(re.sub(",", "\n", "a,b,c"), re.sub(r"a|", "-", "bab"), re.sub("", "-", "ab"))
try:
    re.compile("(abc")
except re.error as e:
    print("error", type(e).__name__)
print(re.search(r"[^\d\s]+", "12 ab3"), re.search(r"[\w.-]+@[\w.-]+", "mail: a.b-c@x.org!").group())
print(re.sub(r"^\s+|\s+$", "", "  pad  "), re.findall(r"[A-Z][a-z]*", "HelloWorld Foo"), re.findall(r"(\w)(\d)", "a1b2"))
print(re.sub(r"(?m)^(G1)", r"\1;", "G1 X\nG1 Y"), re.search(r"(?s)a.b", "a\nb").group(), re.search(r"a.b", "a\nb"))
