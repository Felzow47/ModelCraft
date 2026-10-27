// Small C ABI over web-ifc (That Open Company, MPL-2.0, linked statically, unmodified) for
// ModelCraft: parses IFC2x3 / IFC4 / IFC4x3 from a memory buffer, triangulates every building
// element and hands the placed geometries (metres, Y up, model moved near the origin) to Rust.
#include "parsing/IfcLoader.h"
#include "schema/IfcSchemaManager.h"
#include "schema/ifc-schema.h"
#include "geometry/IfcGeometryProcessor.h"
#include <spdlog/spdlog.h>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include <exception>
#include <string>
#include <unordered_map>
#include <vector>

extern "C" {
// returns the element index Rust assigned, < 0 to stop
typedef int (*wi_elem_cb)(void* user, unsigned id, const char* type, const char* name, const char* storey);
typedef void (*wi_geom_cb)(void* user, int elem, unsigned geom_id, const double* rgba, const double* m16,
                           const float* verts6, unsigned nverts, const unsigned* idx, unsigned nidx);
typedef int (*wi_prog_cb)(void* user, unsigned done, unsigned total);
}

namespace {
std::string str_arg(webifc::parsing::IfcLoader& L, unsigned id, unsigned arg) {
    try {
        if (L.GetNoLineArguments(id) <= arg) return {};
        L.MoveToLineArgument(id, arg);
        if (L.GetTokenType() != webifc::parsing::IfcTokenType::STRING) return {};
        L.StepBack();
        return L.GetDecodedStringArgument();
    } catch (...) {
        return {};
    }
}
void set_err(char* err, int n, const std::string& s) {
    if (err && n > 0) std::snprintf(err, (size_t)n, "%s", s.c_str());
}
} // namespace

extern "C" int wi_load(const char* data, size_t len, void* user, wi_elem_cb elem, wi_geom_cb geom, wi_prog_cb prog,
                       char* schema_out, int schema_len, char* err, int errlen) {
    try {
        // web-ifc reports recoverable geometry issues on stderr; the GUI has no console
        spdlog::set_level(spdlog::level::off);
        webifc::schema::IfcSchemaManager schemas;
        webifc::parsing::IfcLoader loader(67108864, 4294967295ull, 10000, schemas);
        loader.LoadFile([&](char* dest, size_t off, size_t size) -> uint32_t {
            if (off >= len) return 0;
            size_t n = std::min(len - off, size);
            std::memcpy(dest, data + off, n);
            return (uint32_t)n;
        });
        if (schema_out && schema_len > 0) {
            std::string sn;
            try {
                sn = std::string(schemas.GetSchemaName(loader.GetSchema()));
            } catch (...) {
            }
            std::snprintf(schema_out, (size_t)schema_len, "%s", sn.c_str());
        }
        webifc::geometry::IfcGeometryProcessor gp(loader, schemas, 12, true, 1.0E-01, 3.0E-04, 3.0E-04, 1.0E-10, 1.0E-04, 10, 150);

        // element -> storey name
        std::unordered_map<unsigned, std::string> storey_of;
        for (unsigned rel : loader.GetExpressIDsWithType(webifc::schema::IFCRELCONTAINEDINSPATIALSTRUCTURE)) {
            try {
                loader.MoveToLineArgument(rel, 5);
                unsigned st = loader.GetRefArgument();
                std::string sname = str_arg(loader, st, 2);
                if (sname.empty()) sname = schemas.IfcTypeCodeToType(loader.GetLineType(st));
                loader.MoveToLineArgument(rel, 4);
                auto items = loader.GetSetArgument();
                for (auto off : items) {
                    unsigned e = loader.GetRefArgument(off);
                    storey_of[e] = sname;
                }
            } catch (...) {
            }
        }

        std::vector<std::pair<unsigned, unsigned>> todo; // (type, id)
        for (unsigned type : schemas.GetIfcElementList()) {
            if (type == webifc::schema::IFCOPENINGELEMENT || type == webifc::schema::IFCSPACE) continue;
            for (unsigned id : loader.GetExpressIDsWithType(type)) todo.push_back({type, id});
        }
        std::sort(todo.begin(), todo.end(), [](auto& a, auto& b) { return a.second < b.second; });
        unsigned done = 0;
        for (auto [type, id] : todo) {
            if (prog && (done % 64 == 0) && prog(user, done, (unsigned)todo.size()) != 0) {
                set_err(err, errlen, "annulé");
                return 9;
            }
            ++done;
            webifc::geometry::IfcFlatMesh fm;
            try {
                fm = gp.GetFlatMesh(id);
            } catch (...) {
                continue;
            }
            if (fm.geometries.empty()) continue;
            std::string tname = schemas.IfcTypeCodeToType(type);
            std::string name = str_arg(loader, id, 2);
            auto it = storey_of.find(id);
            int ei = elem(user, id, tname.c_str(), name.c_str(), it == storey_of.end() ? "" : it->second.c_str());
            if (ei < 0) {
                set_err(err, errlen, "annulé");
                return 9;
            }
            for (auto& pg : fm.geometries) {
                try {
                    auto& g = gp.GetGeometry(pg.geometryExpressID);
                    const float* v = reinterpret_cast<const float*>(g.GetVertexData()); // fills the float copy
                    unsigned nv = g.GetVertexDataSize() / 6;
                    unsigned ni = g.GetIndexDataSize();
                    if (!v || nv == 0 || ni < 3) continue;
                    const unsigned* ix = reinterpret_cast<const unsigned*>(g.GetIndexData());
                    double rgba[4] = {pg.color.r, pg.color.g, pg.color.b, pg.color.a};
                    geom(user, ei, pg.geometryExpressID, rgba, pg.flatTransformation.data(), v, nv, ix, ni);
                } catch (...) {
                }
            }
        }
        return 0;
    } catch (const std::exception& e) {
        set_err(err, errlen, e.what());
        return 3;
    } catch (...) {
        set_err(err, errlen, "exception inconnue");
        return 4;
    }
}
