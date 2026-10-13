// Differential test driver: boost::geometry::index::rtree<V, quadratic<8>> packed from a range and
// queried with nearest(q, 1) (as in CuraEngine's SpatialLookup). Input: "N" then N points, then
// "Q" lines with query points; output: the index of the nearest value per query.
#include <boost/geometry/algorithms/comparable_distance.hpp>
#include <boost/geometry/core/cs.hpp>
#include <boost/geometry/geometries/register/point.hpp>
#include <boost/geometry/index/rtree.hpp>
#include <cstdio>
#include <iostream>
#include <vector>
struct P { double x, y, z; };
struct V { P position; int idx; };
BOOST_GEOMETRY_REGISTER_POINT_3D_CONST(V, double, boost::geometry::cs::cartesian, position.x, position.y, position.z)
BOOST_GEOMETRY_REGISTER_POINT_3D(P, double, boost::geometry::cs::cartesian, x, y, z)
int main()
{
    size_t n;
    std::cin >> n;
    std::vector<V> values(n);
    for (size_t i = 0; i < n; i++) { std::cin >> values[i].position.x >> values[i].position.y >> values[i].position.z; values[i].idx = int(i); }
    using Tree = boost::geometry::index::rtree<V, boost::geometry::index::quadratic<8>>;
    Tree tree(values.begin(), values.end());
    P q;
    while (std::cin >> q.x >> q.y >> q.z)
    {
        std::vector<V> res;
        tree.query(boost::geometry::index::nearest(q, 1), std::back_inserter(res));
        printf("%d\n", res.empty() ? -1 : res.front().idx);
    }
}
