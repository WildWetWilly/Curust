#include <boost/polygon/voronoi.hpp>
#include <cstdio>
#include <vector>
using namespace boost::polygon;
int main() {
  int np, ns; 
  if (scanf("%d", &np) != 1) return 1;
  voronoi_builder<int> vb;
  for (int i = 0; i < np; i++) { long long x, y; if (scanf("%lld %lld", &x, &y) != 2) return 1; vb.insert_point((int)x, (int)y); }
  if (scanf("%d", &ns) != 1) return 1;
  for (int i = 0; i < ns; i++) { long long a, b, c, d; if (scanf("%lld %lld %lld %lld", &a, &b, &c, &d) != 4) return 1; vb.insert_segment((int)a, (int)b, (int)c, (int)d); }
  voronoi_diagram<double> vd;
  vb.construct(&vd);
  const auto* e0 = vd.edges().empty() ? nullptr : &vd.edges()[0];
  const auto* v0 = vd.vertices().empty() ? nullptr : &vd.vertices()[0];
  const auto* c0 = &vd.cells()[0];
  auto ei = [&](const voronoi_edge<double>* e) -> long { return e ? (long)(e - e0) : -1; };
  auto vi = [&](const voronoi_vertex<double>* v) -> long { return v ? (long)(v - v0) : -1; };
  printf("cells %zu\n", vd.num_cells());
  for (auto& c : vd.cells()) printf("c %zu %d %ld %zu\n", c.source_index(), (int)c.source_category(), ei(c.incident_edge()), c.color());
  printf("vertices %zu\n", vd.num_vertices());
  for (auto& v : vd.vertices()) printf("v %a %a %ld\n", v.x(), v.y(), ei(v.incident_edge()));
  printf("edges %zu\n", vd.num_edges());
  for (auto& e : vd.edges()) printf("e %ld %ld %ld %ld %ld %d %d\n", (long)(e.cell() - c0), vi(e.vertex0()), ei(e.twin()), ei(e.next()), ei(e.prev()), (int)e.is_linear(), (int)e.is_primary());
}
