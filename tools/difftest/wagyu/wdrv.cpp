#include <mapbox/geometry/wagyu/wagyu.hpp>
#include <cstdio>
#include <cstdint>
#include <vector>
using T = int64_t;
int main(int argc, char** argv) {
    // input: nrings, then per ring: n x y ...; optional clip type in argv[1] (0 inter,1 union,2 diff,3 xor), fill in argv[2]
    int ct = argc > 1 ? atoi(argv[1]) : 1;
    int ft = argc > 2 ? atoi(argv[2]) : 0;
    mapbox::geometry::wagyu::wagyu<T> w;
    int nr;
    if (scanf("%d", &nr) != 1) return 1;
    for (int r = 0; r < nr; r++) {
        int n, pt;
        scanf("%d %d", &pt, &n);
        mapbox::geometry::linear_ring<T> ring;
        for (int i = 0; i < n; i++) { long long x, y; scanf("%lld %lld", &x, &y); ring.emplace_back(x, y); }
        bool ok = w.add_ring(ring, pt ? mapbox::geometry::wagyu::polygon_type_clip : mapbox::geometry::wagyu::polygon_type_subject);
        printf("add %d\n", ok ? 1 : 0);
    }
    mapbox::geometry::multi_polygon<T> sln;
    try {
        bool ok = w.execute((mapbox::geometry::wagyu::clip_type)ct, sln, (mapbox::geometry::wagyu::fill_type)ft, (mapbox::geometry::wagyu::fill_type)ft);
        printf("exec %d\n", ok ? 1 : 0);
    } catch (std::exception& e) { printf("exception\n"); return 0; }
    printf("%zu\n", sln.size());
    for (auto& p : sln) {
        printf("%zu\n", p.size());
        for (auto& r : p) { printf("%zu", r.size()); for (auto& q : r) printf(" %lld %lld", (long long)q.x, (long long)q.y); printf("\n"); }
    }
}
