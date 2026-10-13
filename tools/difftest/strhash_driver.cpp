// libstdc++ std::unordered_map<std::string, int> iteration order for random keys (seed = argv[1]);
// compare with `cargo run -p cppstd --example strhash_driver` fed the first output line.
#include <unordered_map>
#include <string>
#include <iostream>
#include <random>
int main(int argc, char** argv) {
    std::mt19937 rng(atoi(argv[1]));
    std::unordered_map<std::string, int> m;
    int n = rng() % 40 + 1;
    for (int i = 0; i < n; i++) {
        std::string s;
        int kind = rng() % 3;
        if (kind == 0) s = "extruder." + std::to_string(rng() % 16);
        else if (kind == 1) s = "model" + std::to_string(rng() % 100) + ".stl";
        else { int l = rng() % 20; for (int j = 0; j < l; j++) s += char('a' + rng() % 26); }
        std::cout << s << "\t";
        m[s] = i;
    }
    std::cout << "\n";
    for (auto& [k, v] : m) std::cout << k << "\t";
    std::cout << "\n" << std::hash<std::string>{}("extruder.0") << "\n";
}
