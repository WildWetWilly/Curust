#!/bin/bash
# Builds an instrumentable reference CuraEngine 5.12 from source (CommandLine only).
set -e
cd "$(dirname "$0")"
D=${DEPS:-$HOME/deps}
W=${WAGYU_INCLUDE:-$D/wagyu/include}
CXXFLAGS="-std=c++20 -O3 -DNDEBUG -w -pthread -Iinclude -Istubs -I$D/fmt/include -I$D/spdlog/include -I$D/range-v3/include -I$D/rapidjson/include -I$D/stb -I$D/boost-1.86.0 -I$W -I$D/geometry.hpp/include -I$D/variant/include -I$D/semver/include -DSPDLOG_FMT_EXTERNAL -DFMT_HEADER_ONLY -DCURA_ENGINE_VERSION=\"5.12.0\" -DCURA_ENGINE_HASH=\"unknown\" -DSENTRY_ENVIRONMENT=\"\" -DVERSION=\"5.12.0\" $EXTRA_CXXFLAGS"
mkdir -p obj
SRCS=$( (grep -vE "Arcus|Emscripten|plugins/" srcs.txt; echo src/main.cpp; echo clipper.cpp) )
compile() {
  src=$1; obj=obj/$(echo $src | tr '/' '_' ).o
  if [ ! -f $obj ] || [ $src -nt $obj ] || [ -n "$FORCE" ]; then
    g++ $CXXFLAGS -c $src -o $obj || { echo "FAILED $src"; return 1; }
  fi
}
export -f compile; export CXXFLAGS
echo "$SRCS" | xargs -P ${JOBS:-8} -I{} bash -c 'compile {}'
g++ -pthread -o curaengine_ref obj/*.o -lpng -lz
echo built curaengine_ref
