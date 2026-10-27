// Small C ABI over Open Asset Import Library (assimp, BSD-3-Clause, linked statically) for
// ModelCraft: imports FBX, glTF, COLLADA, 3MF, PLY, IFC, X3D, USD… and hands the node tree and the
// triangulated meshes to Rust through callbacks. Geometry stays in file units; Rust scales it.
#include <assimp/Importer.hpp>
#include <assimp/ProgressHandler.hpp>
#include <assimp/scene.h>
#include <assimp/postprocess.h>
#include <assimp/config.h>
#include <assimp/material.h>
#include <assimp/DefaultLogger.hpp>
#include <cstring>
#include <cstdio>
#include <exception>
#include <string>
#include <vector>

extern "C" {
typedef int (*as_node_cb)(void* user, const char* name, int parent, const float* local16, int nmeshes);
typedef void (*as_mesh_cb)(void* user, int node, int mesh_id, const char* name, int has_col, const float* rgba,
                           const float* pos, int nv, const unsigned* idx, int nt);
typedef void (*as_meta_cb)(void* user, const char* key, double value);
typedef int (*as_prog_cb)(void* user, float fraction);
}

namespace {
struct Prog : Assimp::ProgressHandler {
    void* user;
    as_prog_cb cb;
    Prog(void* u, as_prog_cb c) : user(u), cb(c) {}
    bool Update(float percentage) override { return cb ? cb(user, percentage < 0 ? 0.f : percentage) == 0 : true; }
};

void set_err(char* err, int errlen, const std::string& s) {
    if (err && errlen > 0) {
        std::snprintf(err, (size_t)errlen, "%s", s.c_str());
    }
}

bool material_color(const aiMaterial* m, float rgba[4]) {
    if (!m) return false;
    aiColor4D c;
    bool ok = false;
    if (m->Get(AI_MATKEY_BASE_COLOR, c) == AI_SUCCESS) ok = true;
    else if (m->Get(AI_MATKEY_COLOR_DIFFUSE, c) == AI_SUCCESS) ok = true;
    if (!ok) return false;
    rgba[0] = c.r; rgba[1] = c.g; rgba[2] = c.b; rgba[3] = c.a;
    float op = 1.f;
    if (m->Get(AI_MATKEY_OPACITY, op) == AI_SUCCESS && op >= 0.f && op <= 1.f && op < rgba[3]) rgba[3] = op;
    if (rgba[3] <= 0.01f) rgba[3] = 1.f; // exporters often write alpha 0 for opaque materials
    return true;
}

struct Walk {
    const aiScene* sc;
    void* user;
    as_node_cb node;
    as_mesh_cb mesh;
    std::vector<unsigned> idx;
    std::vector<float> pos;

    void visit(const aiNode* n, int parent) {
        float m[16];
        const aiMatrix4x4& t = n->mTransformation; // row-major, translation in a4 b4 c4
        // column-major for Rust
        m[0] = t.a1; m[1] = t.b1; m[2] = t.c1; m[3] = t.d1;
        m[4] = t.a2; m[5] = t.b2; m[6] = t.c2; m[7] = t.d2;
        m[8] = t.a3; m[9] = t.b3; m[10] = t.c3; m[11] = t.d3;
        m[12] = t.a4; m[13] = t.b4; m[14] = t.c4; m[15] = t.d4;
        int me = node(user, n->mName.C_Str(), parent, m, (int)n->mNumMeshes);
        if (me < 0) return; // Rust asked to stop
        for (unsigned k = 0; k < n->mNumMeshes; ++k) {
            unsigned mi = n->mMeshes[k];
            if (mi >= sc->mNumMeshes) continue;
            const aiMesh* am = sc->mMeshes[mi];
            if (!am || !am->mVertices || !(am->mPrimitiveTypes & aiPrimitiveType_TRIANGLE)) continue;
            idx.clear();
            idx.reserve((size_t)am->mNumFaces * 3);
            for (unsigned f = 0; f < am->mNumFaces; ++f) {
                const aiFace& fc = am->mFaces[f];
                if (fc.mNumIndices != 3) continue;
                idx.push_back(fc.mIndices[0]);
                idx.push_back(fc.mIndices[1]);
                idx.push_back(fc.mIndices[2]);
            }
            if (idx.empty()) continue;
            pos.resize((size_t)am->mNumVertices * 3);
            for (unsigned v = 0; v < am->mNumVertices; ++v) {
                pos[v * 3] = am->mVertices[v].x;
                pos[v * 3 + 1] = am->mVertices[v].y;
                pos[v * 3 + 2] = am->mVertices[v].z;
            }
            float rgba[4] = {0.7f, 0.7f, 0.7f, 1.f};
            const aiMaterial* mat = am->mMaterialIndex < sc->mNumMaterials ? sc->mMaterials[am->mMaterialIndex] : nullptr;
            int has = material_color(mat, rgba) ? 1 : 0;
            std::string name = am->mName.C_Str();
            if (name.empty() && mat) {
                aiString mn;
                if (mat->Get(AI_MATKEY_NAME, mn) == AI_SUCCESS) name = mn.C_Str();
            }
            mesh(user, me, (int)mi, name.c_str(), has, rgba, pos.data(), (int)am->mNumVertices, idx.data(), (int)(idx.size() / 3));
        }
        for (unsigned c = 0; c < n->mNumChildren; ++c) visit(n->mChildren[c], me);
    }
};
} // namespace

extern "C" int as_load(const char* path, void* user, as_node_cb node, as_mesh_cb mesh, as_meta_cb meta, as_prog_cb prog,
                       char* err, int errlen) {
    try {
        Assimp::Importer imp;
        imp.SetPropertyInteger(AI_CONFIG_PP_SBP_REMOVE, aiPrimitiveType_POINT | aiPrimitiveType_LINE);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_PRESERVE_PIVOTS, false);
        imp.SetPropertyBool(AI_CONFIG_FBX_CONVERT_TO_M, true);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_IFC_SKIP_SPACE_REPRESENTATIONS, true);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_READ_CAMERAS, false);
        imp.SetPropertyBool(AI_CONFIG_IMPORT_FBX_READ_LIGHTS, false);
        imp.SetProgressHandler(new Prog(user, prog)); // owned by the importer
        unsigned flags = aiProcess_Triangulate | aiProcess_JoinIdenticalVertices | aiProcess_SortByPType |
                         aiProcess_FindDegenerates | aiProcess_FindInvalidData | aiProcess_ValidateDataStructure;
        const aiScene* sc = imp.ReadFile(path, flags);
        if (!sc) {
            set_err(err, errlen, imp.GetErrorString());
            return 1;
        }
        if (!sc->mRootNode) {
            set_err(err, errlen, "scène vide");
            return 2;
        }
        if (sc->mMetaData && meta) {
            const aiMetadata* md = sc->mMetaData;
            for (unsigned i = 0; i < md->mNumProperties; ++i) {
                const aiMetadataEntry& e = md->mValues[i];
                double v = 0;
                bool num = true;
                switch (e.mType) {
                case AI_BOOL: v = *(bool*)e.mData ? 1 : 0; break;
                case AI_INT32: v = *(int32_t*)e.mData; break;
                case AI_UINT64: v = (double)*(uint64_t*)e.mData; break;
                case AI_FLOAT: v = *(float*)e.mData; break;
                case AI_DOUBLE: v = *(double*)e.mData; break;
                case AI_INT64: v = (double)*(int64_t*)e.mData; break;
                case AI_UINT32: v = *(uint32_t*)e.mData; break;
                default: num = false;
                }
                if (num) meta(user, md->mKeys[i].C_Str(), v);
            }
        }
        Walk w{sc, user, node, mesh, {}, {}};
        w.visit(sc->mRootNode, -1);
        return 0;
    } catch (const std::exception& e) {
        set_err(err, errlen, e.what());
        return 3;
    } catch (...) {
        set_err(err, errlen, "exception inconnue");
        return 4;
    }
}
