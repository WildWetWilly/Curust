// Differential test driver: libstdc++ std::unordered_set<Point3LL> with CuraEngine's std::hash.
#include <cstdio>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>
#include <unordered_set>
struct P3 { long long x, y, z; bool operator==(const P3& o) const { return x == o.x && y == o.y && z == o.z; } };
namespace std {
template<> struct hash<P3> {
    size_t operator()(const P3& pp) const noexcept {
        static int prime = 31;
        int result = 89;
        result = static_cast<int>(result * prime + pp.x);
        result = static_cast<int>(result * prime + pp.y);
        result = static_cast<int>(result * prime + pp.z);
        return static_cast<size_t>(result);
    }
};
}
int main() {
    std::unordered_set<P3> sets[4];
    std::string line;
    while (std::getline(std::cin, line)) {
        std::istringstream ss(line);
        std::string op; ss >> op;
        if (op == "I" || op == "E") { int id; P3 p; ss >> id >> p.x >> p.y >> p.z; if (op == "I") sets[id].emplace(p); else sets[id].erase(p); }
        else if (op == "M") { int a, b; ss >> a >> b; sets[a].merge(sets[b]); }
        else if (op == "P") { int id; ss >> id; printf("%zu %zu:", sets[id].size(), sets[id].bucket_count()); for (auto& p : sets[id]) printf(" %lld,%lld,%lld", p.x, p.y, p.z); printf("\n"); }
    }
}
