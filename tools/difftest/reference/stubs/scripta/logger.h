#pragma once
// Stub of Ultimaker's scripta debugging logger (no-op).
namespace scripta
{
template<typename F>
struct CellVDI
{
    const char* name;
    F func;
};
template<typename F>
CellVDI(const char*, F) -> CellVDI<F>;
template<typename F>
struct PointVDI
{
    const char* name;
    F func;
};
template<typename F>
PointVDI(const char*, F) -> PointVDI<F>;
template<typename... Args>
inline void log(Args&&...)
{
}
template<typename... Args>
inline void setAll(Args&&...)
{
}
} // namespace scripta
