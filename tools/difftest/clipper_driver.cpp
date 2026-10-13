// Differential test driver for ClipperLib 6.4.2 (C++ reference).
#include "clipper.hpp"
#include <iostream>
#include <sstream>
#include <string>
#include <vector>
#include <algorithm>
using namespace ClipperLib;

static Path readPath(std::istringstream& ss) {
  int n; ss >> n; Path p(n);
  for (int i = 0; i < n; i++) ss >> p[i].X >> p[i].Y;
  return p;
}
static void printPath(const Path& p) {
  for (size_t i = 0; i < p.size(); i++) std::cout << (i ? " " : "") << p[i].X << " " << p[i].Y;
  std::cout << "\n";
}
static void printTree(const PolyTree& t) {
  PolyNode* n = t.GetFirst();
  while (n) {
    int depth = 0; for (PolyNode* p = n->Parent; p; p = p->Parent) depth++;
    std::cout << "N " << depth << " " << n->IsOpen() << " " << n->IsHole() << " ";
    printPath(n->Contour);
    n = n->GetNext();
  }
}
int main() {
  std::string line;
  while (std::getline(std::cin, line)) {
    std::istringstream hs(line); std::string mode; hs >> mode;
    if (mode == "clip") {
      int ct, sf, cf, strict, tree, preserve, rev; hs >> ct >> sf >> cf >> strict >> tree >> preserve >> rev;
      Clipper c; c.StrictlySimple(strict); c.PreserveCollinear(preserve); c.ReverseSolution(rev);
      while (std::getline(std::cin, line) && line != "END") {
        std::istringstream ss(line); std::string t; int closed; ss >> t >> closed;
        Path p = readPath(ss);
        c.AddPath(p, t == "S" ? ptSubject : ptClip, closed);
      }
      if (tree) { PolyTree pt; bool ok = c.Execute((ClipType)ct, pt, (PolyFillType)sf, (PolyFillType)cf); std::cout << "OK " << ok << "\n"; printTree(pt); }
      else { Paths sol; bool ok = c.Execute((ClipType)ct, sol, (PolyFillType)sf, (PolyFillType)cf); std::cout << "OK " << ok << "\n"; for (auto& p : sol) printPath(p); }
      std::cout << "DONE" << std::endl;
    } else if (mode == "offset") {
      double delta, ml, at; int tree; hs >> delta >> ml >> at >> tree;
      ClipperOffset o(ml, at);
      while (std::getline(std::cin, line) && line != "END") {
        std::istringstream ss(line); std::string t; int jt, et; ss >> t >> jt >> et;
        Path p = readPath(ss); o.AddPath(p, (JoinType)jt, (EndType)et);
      }
      if (tree) { PolyTree pt; o.Execute(pt, delta); std::cout << "OK 1\n"; printTree(pt); }
      else { Paths sol; o.Execute(sol, delta); std::cout << "OK 1\n"; for (auto& p : sol) printPath(p); }
      std::cout << "DONE" << std::endl;
    } else if (mode == "clean") {
      double d; hs >> d; std::getline(std::cin, line); std::istringstream ss(line); std::string t; ss >> t;
      Path p = readPath(ss); Path out; CleanPolygon(p, out, d); std::getline(std::cin, line);
      std::cout << "OK 1\n"; printPath(out); std::cout << "DONE" << std::endl;
    } else if (mode == "sort") {
      int n; hs >> n; std::vector<std::pair<long long,int>> v(n);
      for (int i = 0; i < n; i++) { hs >> v[i].first; v[i].second = i; }
      std::sort(v.begin(), v.end(), [](auto& a, auto& b){ return a.first < b.first; });
      std::cout << "OK 1\n"; for (int i = 0; i < n; i++) std::cout << (i?" ":"") << v[i].second; std::cout << "\nDONE" << std::endl;
    } else if (mode == "heap") {
      int n; hs >> n; std::vector<std::pair<long long,int>> v(n);
      for (int i = 0; i < n; i++) { hs >> v[i].first; v[i].second = i; }
      auto cmp = [](const std::pair<long long,int>& a, const std::pair<long long,int>& b){ return a.first < b.first; };
      std::priority_queue<std::pair<long long,int>, std::vector<std::pair<long long,int>>, decltype(cmp)> pq(cmp);
      for (auto& x : v) pq.push(x);
      std::cout << "OK 1\n"; bool first = true; while (!pq.empty()) { std::cout << (first?"":" ") << pq.top().second; first=false; pq.pop(); } std::cout << "\nDONE" << std::endl;
    }
  }
}
