#include <map>
#include <cstdio>
#include <cstdint>
#include <unordered_map>
#include <string>
typedef std::map<long, long> M;
static void dump(std::_Rb_tree_node_base* x, std::string& out) {
  if (!x) { out += '.'; return; }
  char buf[64]; snprintf(buf, sizeof buf, "(%ld%c", static_cast<std::_Rb_tree_node<std::pair<const long,long>>*>(x)->_M_valptr()->first, x->_M_color == std::_S_red ? 'r' : 'b');
  out += buf; dump(x->_M_left, out); dump(x->_M_right, out); out += ')';
}
int main() {
  M m; uint64_t x = 987654321ULL; std::unordered_map<long, M::iterator> its;
  auto root = [&]() { return m.begin()._M_node->_M_parent ? (std::_Rb_tree_node_base*)nullptr : nullptr; };
  for (int step = 0; step < 20000; step++) {
    x = x * 6364136223846793005ULL + 1442695040888963407ULL;
    long k = (long)((x >> 33) % 300); int mode = (int)((x >> 20) % 4);
    auto f = its.find(k);
    if (f != its.end()) { m.erase(f->second); its.erase(f); }
    else {
      M::iterator it;
      if (mode == 0) it = m.insert(std::make_pair(k, 0L)).first;
      else if (mode == 1) it = m.insert(m.lower_bound(k), M::value_type(k, 0));
      else if (mode == 2) it = m.insert(m.end(), M::value_type(k, 0));
      else it = m.insert(m.begin(), M::value_type(k, 0));
      its[k] = it;
    }
    if (step % 97 == 0) { std::string s; dump(m.end()._M_node->_M_parent, s); printf("%s\n", s.c_str()); }
  }
  std::string s; dump(m.end()._M_node->_M_parent, s); printf("%s\n", s.c_str());
}
