// Small C ABI over OpenCascade (LGPL-2.1 with exception, linked statically) for ModelCraft:
// reads STEP / IGES with XCAF (assembly tree, names, colours), tessellates every part once and
// hands the faces to Rust through callbacks. Units: millimetres.
#include <STEPCAFControl_Reader.hxx>
#include <IGESCAFControl_Reader.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <TDocStd_Document.hxx>
#include <TDataStd_Name.hxx>
#include <TDF_LabelSequence.hxx>
#include <TDF_ChildIterator.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <BRep_Tool.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepBndLib.hxx>
#include <BRepLib_ToolTriangulatedShape.hxx>
#include <Bnd_Box.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <Quantity_ColorRGBA.hxx>
#include <gp_Trsf.hxx>
#include <Standard_Failure.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Interface_Static.hxx>
#include <map>
#include <string>
#include <vector>
#include <cstring>
#include <cmath>

extern "C" {
typedef int (*oc_node_cb)(void* ctx, const char* name, int parent, int is_asm, const double* m16, int part_key, int has_col, const float* rgba);
typedef void (*oc_face_cb)(void* ctx, int part_key, int has_col, const float* rgba, const float* pos, const float* nrm, int nverts, const unsigned* idx, int ntris);
typedef void (*oc_progress_cb)(void* ctx, int stage, int done, int total);
}

namespace {
struct Ctx {
  void* user;
  oc_node_cb node;
  oc_face_cb face;
  oc_progress_cb progress;
  Handle(XCAFDoc_ShapeTool) st;
  Handle(XCAFDoc_ColorTool) ct;
  double lin, ang;
  std::map<int, int> part_keys;  // label tag hash -> key
  int next_key = 0;
  int parts_done = 0, parts_total = 0;
};

std::string label_name(const TDF_Label& l) {
  Handle(TDataStd_Name) n;
  if (l.FindAttribute(TDataStd_Name::GetID(), n)) {
    TCollection_AsciiString s(n->Get(), '?');
    // ExtendedString -> UTF-8
    char* buf = new char[n->Get().LengthOfCString() + 1];
    Standard_PCharacter p = buf;
    n->Get().ToUTF8CString(p);
    std::string r(buf);
    delete[] buf;
    return r;
  }
  return std::string();
}

bool get_color(Ctx& c, const TDF_Label& l, Quantity_ColorRGBA& col) {
  return c.ct->GetColor(l, XCAFDoc_ColorSurf, col) || c.ct->GetColor(l, XCAFDoc_ColorGen, col);
}
bool get_shape_color(Ctx& c, const TopoDS_Shape& s, Quantity_ColorRGBA& col) {
  return c.ct->GetColor(s, XCAFDoc_ColorSurf, col) || c.ct->GetColor(s, XCAFDoc_ColorGen, col);
}

void to_rgba(const Quantity_ColorRGBA& c, float* out) {
  NCollection_Vec3<float> v = c.GetRGB().Rgb(); // linear RGB
  Standard_Real r, g, b;
  c.GetRGB().Values(r, g, b, Quantity_TOC_sRGB);
  out[0] = (float)r; out[1] = (float)g; out[2] = (float)b; out[3] = c.Alpha();
  (void)v;
}

void trsf_to_m16(const gp_Trsf& t, double* m) {
  for (int col = 0; col < 3; ++col)
    for (int row = 0; row < 3; ++row) m[col * 4 + row] = t.Value(row + 1, col + 1);
  m[3] = m[7] = m[11] = 0;
  m[12] = t.Value(1, 4); m[13] = t.Value(2, 4); m[14] = t.Value(3, 4); m[15] = 1;
}

void mesh_part(Ctx& c, const TDF_Label& part, int key) {
  TopoDS_Shape shape = c.st->GetShape(part);
  if (shape.IsNull()) return;
  try {
    BRepMesh_IncrementalMesh mesher(shape, c.lin, Standard_False, c.ang, Standard_True);
  } catch (Standard_Failure&) {
    return;
  }
  Quantity_ColorRGBA part_col;
  bool has_part_col = get_color(c, part, part_col);
  // sub-shape colours (faces / solids) registered under the part label
  std::vector<std::pair<TopoDS_Shape, Quantity_ColorRGBA>> sub;
  TDF_LabelSequence subs;
  c.st->GetSubShapes(part, subs);
  for (int i = 1; i <= subs.Length(); ++i) {
    Quantity_ColorRGBA col;
    if (get_color(c, subs.Value(i), col)) sub.push_back({c.st->GetShape(subs.Value(i)), col});
  }
  std::vector<float> pos, nrm;
  std::vector<unsigned> idx;
  for (TopExp_Explorer ex(shape, TopAbs_FACE); ex.More(); ex.Next()) {
    const TopoDS_Face& f = TopoDS::Face(ex.Current());
    TopLoc_Location loc;
    Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(f, loc);
    if (tri.IsNull() || tri->NbTriangles() == 0) continue;
    if (!tri->HasNormals()) BRepLib_ToolTriangulatedShape::ComputeNormals(f, tri);
    gp_Trsf tr = loc.Transformation();
    bool rev = f.Orientation() == TopAbs_REVERSED;
    int nv = tri->NbNodes();
    pos.resize(nv * 3); nrm.resize(nv * 3);
    for (int i = 1; i <= nv; ++i) {
      gp_Pnt p = tri->Node(i).Transformed(tr);
      pos[(i - 1) * 3] = (float)p.X(); pos[(i - 1) * 3 + 1] = (float)p.Y(); pos[(i - 1) * 3 + 2] = (float)p.Z();
      gp_Dir d(0, 0, 1);
      if (tri->HasNormals()) {
        gp_Vec v(tri->Normal(i));
        if (v.Magnitude() > 1e-12) d = gp_Dir(v).Transformed(tr);
      }
      if (rev) d.Reverse();
      nrm[(i - 1) * 3] = (float)d.X(); nrm[(i - 1) * 3 + 1] = (float)d.Y(); nrm[(i - 1) * 3 + 2] = (float)d.Z();
    }
    int nt = tri->NbTriangles();
    idx.resize(nt * 3);
    for (int i = 1; i <= nt; ++i) {
      int a, b, cc;
      tri->Triangle(i).Get(a, b, cc);
      if (rev) std::swap(b, cc);
      idx[(i - 1) * 3] = a - 1; idx[(i - 1) * 3 + 1] = b - 1; idx[(i - 1) * 3 + 2] = cc - 1;
    }
    Quantity_ColorRGBA col;
    bool has = false;
    for (auto& s : sub) {
      if (s.first.IsSame(f)) { col = s.second; has = true; break; }
    }
    if (!has) {
      for (auto& s : sub) {
        if (s.first.ShapeType() < TopAbs_FACE) {
          for (TopExp_Explorer e2(s.first, TopAbs_FACE); e2.More() && !has; e2.Next())
            if (e2.Current().IsSame(f)) { col = s.second; has = true; }
          if (has) break;
        }
      }
    }
    if (!has) has = get_shape_color(c, f, col);
    if (!has && has_part_col) { col = part_col; has = true; }
    float rgba[4] = {0.8f, 0.8f, 0.8f, 1.0f};
    if (has) to_rgba(col, rgba);
    c.face(c.user, key, has ? 1 : 0, rgba, pos.data(), nrm.data(), nv, idx.data(), nt);
  }
}

int part_key(Ctx& c, const TDF_Label& part) {
  int tag = part.Tag();
  // labels of shapes are direct children of the shapes label: the tag is unique
  auto it = c.part_keys.find(tag);
  if (it != c.part_keys.end()) return it->second;
  int key = c.next_key++;
  c.part_keys[tag] = key;
  mesh_part(c, part, key);
  c.parts_done++;
  if (c.progress) c.progress(c.user, 2, c.parts_done, c.parts_total);
  return key;
}

void walk(Ctx& c, const TDF_Label& label, int parent, const gp_Trsf& local, const std::string& inst_name) {
  TDF_Label ref = label;
  if (c.st->IsReference(label)) c.st->GetReferredShape(label, ref);
  std::string name = inst_name.empty() ? label_name(ref) : inst_name;
  double m[16];
  trsf_to_m16(local, m);
  Quantity_ColorRGBA col;
  float rgba[4] = {0, 0, 0, 0};
  int has_col = get_color(c, label, col) ? 1 : 0;
  if (has_col) to_rgba(col, rgba);
  if (c.st->IsAssembly(ref)) {
    int me = c.node(c.user, name.c_str(), parent, 1, m, -1, has_col, rgba);
    TDF_LabelSequence comps;
    c.st->GetComponents(ref, comps, Standard_False);
    for (int i = 1; i <= comps.Length(); ++i) {
      TDF_Label comp = comps.Value(i);
      TopLoc_Location loc = c.st->GetLocation(comp);
      std::string cn = label_name(comp);
      walk(c, comp, me, loc.Transformation(), cn);
    }
  } else {
    int key = part_key(c, ref);
    c.node(c.user, name.c_str(), parent, 0, m, key, has_col, rgba);
  }
}
}  // namespace

extern "C" int oc_load(const char* path, int iges, double rel_quality, void* user, oc_node_cb node, oc_face_cb face, oc_progress_cb progress, char* err, int errlen) {
  auto fail = [&](const char* msg) {
    if (err && errlen > 0) { std::strncpy(err, msg, errlen - 1); err[errlen - 1] = 0; }
    return -1;
  };
  try {
    // keep OCCT quiet on stdout
    Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_Printer));
    Handle(XCAFApp_Application) app = XCAFApp_Application::GetApplication();
    Handle(TDocStd_Document) doc;
    app->NewDocument("MDTV-XCAF", doc);
    if (progress) progress(user, 0, 0, 1);
    if (iges) {
      IGESCAFControl_Reader r;
      r.SetColorMode(true); r.SetNameMode(true); r.SetLayerMode(false);
      if (r.ReadFile(path) != IFSelect_RetDone) return fail("lecture IGES impossible (fichier invalide ?)");
      if (progress) progress(user, 1, 0, 1);
      if (!r.Transfer(doc)) return fail("conversion IGES impossible");
    } else {
      STEPCAFControl_Reader r;
      r.SetColorMode(true); r.SetNameMode(true); r.SetLayerMode(false); r.SetPropsMode(false);
      if (r.ReadFile(path) != IFSelect_RetDone) return fail("lecture STEP impossible (fichier invalide ?)");
      if (progress) progress(user, 1, 0, 1);
      if (!r.Transfer(doc)) return fail("conversion STEP impossible");
    }
    Ctx c;
    c.user = user; c.node = node; c.face = face; c.progress = progress;
    c.st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    c.ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());
    TDF_LabelSequence roots;
    c.st->GetFreeShapes(roots);
    if (roots.Length() == 0) return fail("aucune forme dans le fichier");
    Bnd_Box box;
    for (int i = 1; i <= roots.Length(); ++i) BRepBndLib::Add(c.st->GetShape(roots.Value(i)), box);
    double diag = box.IsVoid() ? 100.0 : std::sqrt(box.SquareExtent());
    // quality 0..1 -> linear deflection 1/250 .. 1/4000 of the diagonal
    double q = rel_quality < 0 ? 0 : (rel_quality > 1 ? 1 : rel_quality);
    c.lin = diag / (250.0 * std::pow(16.0, q));
    c.ang = 0.6 - 0.35 * q;
    TDF_LabelSequence shapes;
    c.st->GetShapes(shapes);
    for (int i = 1; i <= shapes.Length(); ++i)
      if (!c.st->IsAssembly(shapes.Value(i))) c.parts_total++;
    for (int i = 1; i <= roots.Length(); ++i) walk(c, roots.Value(i), -1, gp_Trsf(), std::string());
    app->Close(doc);
    return 0;
  } catch (Standard_Failure& e) {
    return fail(e.GetMessageString() ? e.GetMessageString() : "erreur OpenCascade");
  } catch (...) {
    return fail("erreur interne OpenCascade");
  }
}
