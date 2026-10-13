// Differential test driver: boost::concurrent_flat_map<uint64_t, uint8_t> / concurrent_flat_set<uint64_t>
// (Boost 1.86) iteration order. Commands on stdin:
//   A id key val   insert_or_assign       V id key val   insert_or_visit (keeps min)
//   S id key       set insert             E id key       erase (map)
//   F id key       set erase              X id mod rem   map erase_if(key % mod == rem)
//   Y id mod rem   set erase_if            P id / Q id    print map / set
#include <boost/unordered/concurrent_flat_map.hpp>
#include <boost/unordered/concurrent_flat_set.hpp>
#include <cstdint>
#include <cstdio>
#include <iostream>
#include <sstream>
#include <string>
int main()
{
    boost::concurrent_flat_map<uint64_t, uint8_t> maps[2];
    boost::concurrent_flat_set<uint64_t> sets[2];
    std::string line;
    while (std::getline(std::cin, line))
    {
        std::istringstream ss(line);
        std::string op;
        ss >> op;
        int id;
        ss >> id;
        if (op == "A") { uint64_t k; unsigned v; ss >> k >> v; maps[id].insert_or_assign(k, uint8_t(v)); }
        else if (op == "V") { uint64_t k; unsigned v; ss >> k >> v; uint8_t vv = v; maps[id].insert_or_visit({ k, vv }, [vv](auto& e) { e.second = std::min(e.second, vv); }); }
        else if (op == "S") { uint64_t k; ss >> k; sets[id].insert(k); }
        else if (op == "E") { uint64_t k; ss >> k; maps[id].erase(k); }
        else if (op == "F") { uint64_t k; ss >> k; sets[id].erase(k); }
        else if (op == "X") { uint64_t m, r; ss >> m >> r; maps[id].erase_if([&](auto& e) { return e.first % m == r; }); }
        else if (op == "Y") { uint64_t m, r; ss >> m >> r; sets[id].erase_if([&](auto& e) { return e % m == r; }); }
        else if (op == "P") { printf("%zu:", maps[id].size()); maps[id].visit_all([](auto& e) { printf(" %llu=%u", (unsigned long long)e.first, unsigned(e.second)); }); printf("\n"); }
        else if (op == "Q") { printf("%zu:", sets[id].size()); sets[id].visit_all([](auto& e) { printf(" %llu", (unsigned long long)e); }); printf("\n"); }
    }
}
