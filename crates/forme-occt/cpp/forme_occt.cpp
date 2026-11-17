// Forme's thin C layer over OpenCascade (Romain, 17 nov. 2026: robust
// booleans and fillets, and a path to the iPad).
//
// Every entry point catches OpenCascade's exceptions and returns null (or a
// negative count) with the message in focc_last_error(), so a failed fillet
// never takes the app down. Only opencascade::handle<T> is used, never the
// Handle_T aliases (a derived class, not a typedef, under MSVC).

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepLProp_SLProps.hxx>
#include <BRepTools.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepLib_ToolTriangulatedShape.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Tool.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GProp_GProps.hxx>
#include <Geom_BezierCurve.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <Poly_Triangulation.hxx>
#include <Standard_Failure.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Ax2.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>

#include <cstdint>
#include <cstring>
#include <exception>
#include <string>
#include <vector>

struct FShape {
    TopoDS_Shape s;
    // edges of `s` made by the boolean that produced it (1-based indices in
    // the shape's edge map), for junction fillets
    std::vector<int> section;
    // tag of each face, in the order of the shape's face map (-1 = none):
    // the app's name for a face (feature id and which face of it), carried
    // through transforms, booleans and fillets
    std::vector<long long> tags;
};

struct FMesh {
    std::vector<float> pos, nrm;
    std::vector<uint32_t> tri;
};

// polylines, one after the other: counts[i] points each
struct FLines {
    std::vector<double> xyz;
    std::vector<int> counts;
};

static thread_local std::string g_err;

static void set_err(const std::string& s) { g_err = s; }

template <class F>
static auto guard(F f, decltype(f()) fail) -> decltype(f()) {
    try {
        return f();
    } catch (const Standard_Failure& e) {
        const char* m = e.GetMessageString();
        set_err(std::string(e.DynamicType()->Name()) + (m && *m ? std::string(": ") + m : std::string()));
    } catch (const std::exception& e) {
        set_err(e.what());
    } catch (...) {
        set_err("unknown OpenCascade error");
    }
    return fail;
}

static FShape* wrap(const TopoDS_Shape& s) {
    if (s.IsNull()) {
        set_err("empty result");
        return nullptr;
    }
    FShape* r = new FShape();
    r->s = s;
    return r;
}

static void edge_map(const TopoDS_Shape& s, TopTools_IndexedMapOfShape& m) { TopExp::MapShapes(s, TopAbs_EDGE, m); }

static void face_map(const TopoDS_Shape& s, TopTools_IndexedMapOfShape& m) { TopExp::MapShapes(s, TopAbs_FACE, m); }

// Each face of `in` gives its tag to its images in `out` (the faces the
// operation made from it, or itself when it went through unchanged). Faces
// of `out` that come from no tagged face get `fresh`.
static void carry_tags(BRepBuilderAPI_MakeShape& op, const FShape* in, FShape* out, long long fresh) {
    TopTools_IndexedMapOfShape fin, fout;
    face_map(in->s, fin);
    face_map(out->s, fout);
    if ((int)out->tags.size() != fout.Extent()) out->tags.assign(fout.Extent(), -1);
    for (int i = 1; i <= fin.Extent(); i++) {
        long long t = i - 1 < (int)in->tags.size() ? in->tags[i - 1] : -1;
        if (t < 0) continue;
        const TopoDS_Shape& f = fin(i);
        bool any = false;
        const TopTools_ListOfShape& mod = op.Modified(f);
        for (TopTools_ListOfShape::Iterator it(mod); it.More(); it.Next()) {
            int j = fout.FindIndex(it.Value());
            if (j > 0) {
                out->tags[j - 1] = t;
                any = true;
            }
        }
        if (!any) {
            int j = fout.FindIndex(f);
            if (j > 0) out->tags[j - 1] = t;
        }
    }
    for (long long& t : out->tags)
        if (t < 0) t = fresh;
}

// little-endian writer for focc_topo's buffer
struct Buf {
    std::vector<uint8_t> b;
    template <class T>
    void put(T v) {
        size_t n = b.size();
        b.resize(n + sizeof(T));
        std::memcpy(b.data() + n, &v, sizeof(T));
    }
};

// What a face is: kind (0 plane, 1 cylinder, 2 cone, 3 sphere, 4 torus,
// 5 other), a point in its middle and the outward normal there, its axis
// (origin, direction) and radius for round faces.
static int face_info(const TopoDS_Face& f, double* out) {
    for (int k = 0; k < 13; k++) out[k] = 0.0;
    BRepAdaptor_Surface sa(f, Standard_True);
    double u0, u1, v0, v1;
    BRepTools::UVBounds(f, u0, u1, v0, v1);
    double u = (u0 + u1) / 2, v = (v0 + v1) / 2;
    gp_Pnt p = sa.Value(u, v);
    BRepLProp_SLProps props(sa, u, v, 1, 1e-7);
    gp_Dir n(0, 1, 0);
    if (props.IsNormalDefined()) n = props.Normal();
    if (f.Orientation() == TopAbs_REVERSED) n.Reverse();
    out[0] = p.X(); out[1] = p.Y(); out[2] = p.Z();
    out[3] = n.X(); out[4] = n.Y(); out[5] = n.Z();
    int kind = 5;
    gp_Ax1 ax;
    double r = 0.0;
    switch (sa.GetType()) {
        case GeomAbs_Plane: kind = 0; ax = sa.Plane().Axis(); break;
        case GeomAbs_Cylinder: kind = 1; ax = sa.Cylinder().Axis(); r = sa.Cylinder().Radius(); break;
        case GeomAbs_Cone: kind = 2; ax = sa.Cone().Axis(); r = sa.Cone().RefRadius(); break;
        case GeomAbs_Sphere: kind = 3; ax = sa.Sphere().Position().Axis(); r = sa.Sphere().Radius(); break;
        case GeomAbs_Torus: kind = 4; ax = sa.Torus().Axis(); r = sa.Torus().MajorRadius(); break;
        default: ax = gp_Ax1(p, n); break;
    }
    if (kind == 0) ax = gp_Ax1(p, n);
    out[6] = ax.Location().X(); out[7] = ax.Location().Y(); out[8] = ax.Location().Z();
    out[9] = ax.Direction().X(); out[10] = ax.Direction().Y(); out[11] = ax.Direction().Z();
    out[12] = r;
    return kind;
}

static bool valid(const TopoDS_Shape& s) {
    BRepCheck_Analyzer a(s);
    return a.IsValid() == Standard_True;
}

extern "C" {

const char* focc_last_error() { return g_err.c_str(); }

void focc_free(FShape* s) { delete s; }

// ---------------------------------------------------------------- primitives
// All centred on the origin, round shapes along +Y (Forme's convention).

FShape* focc_box(double dx, double dy, double dz) {
    return guard([&] { return wrap(BRepPrimAPI_MakeBox(gp_Pnt(-dx / 2, -dy / 2, -dz / 2), dx, dy, dz).Shape()); }, (FShape*)nullptr);
}

FShape* focc_cylinder(double r, double h) {
    return guard([&] { return wrap(BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(0, -h / 2, 0), gp_Dir(0, 1, 0)), r, h).Shape()); }, (FShape*)nullptr);
}

FShape* focc_sphere(double r) {
    return guard([&] { return wrap(BRepPrimAPI_MakeSphere(gp_Ax2(gp_Pnt(0, 0, 0), gp_Dir(0, 1, 0)), r).Shape()); }, (FShape*)nullptr);
}

FShape* focc_torus(double big, double small) {
    return guard([&] { return wrap(BRepPrimAPI_MakeTorus(gp_Ax2(gp_Pnt(0, 0, 0), gp_Dir(0, 1, 0)), big, small).Shape()); }, (FShape*)nullptr);
}

// Copy of `s` moved by the affine map m (3 rows of 4: rotation and uniform
// scale, possibly a mirror, then translation).
// Closed outline in the XY plane (n points, xy[2n]) extruded along Z
// between -h and +h. A smooth point is rounded into a quadratic curve from
// the middle of its incoming edge to the middle of its outgoing one, like
// the Sketch tool (nurbs-kernel `doc::Profile`).
FShape* focc_prism(const double* xy, const int* smooth, int n, double h) {
    return guard(
        [&]() -> FShape* {
            if (n < 3 || h <= 0) {
                set_err("a profile needs 3 points and a thickness");
                return nullptr;
            }
            std::vector<gp_Pnt> p(n);
            for (int i = 0; i < n; i++) p[i] = gp_Pnt(xy[2 * i], xy[2 * i + 1], -h);
            auto mid = [](const gp_Pnt& a, const gp_Pnt& b) { return gp_Pnt((a.X() + b.X()) / 2, (a.Y() + b.Y()) / 2, (a.Z() + b.Z()) / 2); };
            // where each point's piece starts and ends (the point itself for a corner)
            std::vector<TopoDS_Vertex> vin(n), vout(n);
            for (int i = 0; i < n; i++) {
                const gp_Pnt& prev = p[(i + n - 1) % n];
                const gp_Pnt& next = p[(i + 1) % n];
                bool round = smooth[i] && prev.Distance(p[i]) > 1e-6 && next.Distance(p[i]) > 1e-6;
                if (round) {
                    vin[i] = BRepBuilderAPI_MakeVertex(mid(prev, p[i]));
                    vout[i] = BRepBuilderAPI_MakeVertex(mid(p[i], next));
                } else {
                    vin[i] = vout[i] = BRepBuilderAPI_MakeVertex(p[i]);
                }
            }
            // two smooth points in a row meet at the middle of their edge: one vertex
            for (int i = 0; i < n; i++) {
                int j = (i + 1) % n;
                if (BRep_Tool::Pnt(vout[i]).Distance(BRep_Tool::Pnt(vin[j])) < 1e-7) {
                    bool corner = vin[j].IsSame(vout[j]);
                    vin[j] = vout[i];
                    if (corner) vout[j] = vout[i];
                }
            }
            BRepBuilderAPI_MakeWire mw;
            for (int i = 0; i < n; i++) {
                if (!vin[i].IsSame(vout[i])) {
                    TColgp_Array1OfPnt poles(1, 3);
                    poles(1) = BRep_Tool::Pnt(vin[i]);
                    poles(2) = p[i];
                    poles(3) = BRep_Tool::Pnt(vout[i]);
                    opencascade::handle<Geom_BezierCurve> c = new Geom_BezierCurve(poles);
                    mw.Add(BRepBuilderAPI_MakeEdge(c, vin[i], vout[i]).Edge());
                }
                int j = (i + 1) % n;
                if (!vout[i].IsSame(vin[j])) mw.Add(BRepBuilderAPI_MakeEdge(vout[i], vin[j]).Edge());
            }
            if (!mw.IsDone()) {
                set_err("the outline is not a closed line");
                return nullptr;
            }
            BRepBuilderAPI_MakeFace mf(mw.Wire(), Standard_True);
            if (!mf.IsDone()) {
                set_err("the outline is not flat and closed");
                return nullptr;
            }
            BRepPrimAPI_MakePrism prism(mf.Face(), gp_Vec(0, 0, 2 * h));
            TopoDS_Shape r = prism.Shape();
            if (!valid(r)) {
                set_err("the outline crosses itself");
                return nullptr;
            }
            return wrap(r);
        },
        (FShape*)nullptr);
}

FShape* focc_transform(const FShape* s, const double* m) {
    return guard(
        [&] {
            gp_Trsf t;
            t.SetValues(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]);
            BRepBuilderAPI_Transform op(s->s, t, Standard_True);
            if (!op.IsDone()) {
                set_err("transform failed");
                return (FShape*)nullptr;
            }
            FShape* r = wrap(op.Shape());
            if (r && !s->tags.empty()) {
                carry_tags(op, s, r, -1);
                // same structure: when the history gave nothing, the face
                // order of the copy is the original's
                bool none = true;
                for (long long t : r->tags) none = none && t < 0;
                if (none && r->tags.size() == s->tags.size()) r->tags = s->tags;
            }
            return r;
        },
        (FShape*)nullptr);
}

// ------------------------------------------------------------------ booleans

static FShape* boolean(BRepAlgoAPI_BooleanOperation& op, const char* what, const FShape* a, const FShape* b) {
    op.Build();
    if (!op.IsDone() || op.HasErrors()) {
        set_err(std::string(what) + " failed");
        return nullptr;
    }
    // the junction edges, before the result is simplified
    std::vector<TopoDS_Shape> sec;
    for (TopTools_ListOfShape::Iterator it(op.SectionEdges()); it.More(); it.Next()) sec.push_back(it.Value());
    // merge faces of the same surface (two cylinders stacked end to end
    // become one face, without a seam)
    op.SimplifyResult(Standard_True, Standard_True);
    FShape* r = wrap(op.Shape());
    if (!r) return nullptr;
    carry_tags(op, a, r, -1);
    carry_tags(op, b, r, -1);
    TopTools_IndexedMapOfShape m;
    edge_map(r->s, m);
    for (const TopoDS_Shape& e : sec) {
        int i = m.FindIndex(e);
        if (i > 0) {
            r->section.push_back(i);
            continue;
        }
        for (TopTools_ListOfShape::Iterator it(op.Modified(e)); it.More(); it.Next()) {
            int j = m.FindIndex(it.Value());
            if (j > 0) r->section.push_back(j);
        }
    }
    return r;
}

FShape* focc_fuse(const FShape* a, const FShape* b) {
    return guard(
        [&] {
            BRepAlgoAPI_Fuse op;
            TopTools_ListOfShape args, tools;
            args.Append(a->s);
            tools.Append(b->s);
            op.SetArguments(args);
            op.SetTools(tools);
            return boolean(op, "union", a, b);
        },
        (FShape*)nullptr);
}

FShape* focc_cut(const FShape* a, const FShape* b) {
    return guard(
        [&] {
            BRepAlgoAPI_Cut op;
            TopTools_ListOfShape args, tools;
            args.Append(a->s);
            tools.Append(b->s);
            op.SetArguments(args);
            op.SetTools(tools);
            return boolean(op, "subtraction", a, b);
        },
        (FShape*)nullptr);
}

int focc_section_count(const FShape* s) { return (int)s->section.size(); }

int focc_section_edge(const FShape* s, int k) { return k >= 0 && k < (int)s->section.size() ? s->section[k] : 0; }

// --------------------------------------------------------------------- edges

int focc_edge_count(const FShape* s) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            return m.Extent();
        },
        -1);
}

// Samples edge `index` (1-based) at n points from start to end into xyz.
// Returns its kind: 0 line, 1 circle, 2 other curve, -1 degenerated or error.
int focc_edge_sample(const FShape* s, int index, int n, double* xyz) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            if (index < 1 || index > m.Extent()) return -1;
            TopoDS_Edge e = TopoDS::Edge(m(index));
            if (BRep_Tool::Degenerated(e)) return -1;
            BRepAdaptor_Curve c(e);
            double u0 = c.FirstParameter(), u1 = c.LastParameter();
            for (int i = 0; i < n; i++) {
                double u = n > 1 ? u0 + (u1 - u0) * i / (n - 1) : (u0 + u1) / 2;
                gp_Pnt p = c.Value(u);
                xyz[3 * i] = p.X();
                xyz[3 * i + 1] = p.Y();
                xyz[3 * i + 2] = p.Z();
            }
            switch (c.GetType()) {
                case GeomAbs_Line: return 0;
                case GeomAbs_Circle: return 1;
                default: return 2;
            }
        },
        -1);
}

// For every edge i (1-based, in edge-map order): the 0-based indices of the
// two faces it joins, in out[2(i-1)] and out[2(i-1)+1] (-1 = none: a seam
// joins its face to itself, a free edge has one face). Returns the edge
// count, or -1 (error, or cap too small).
int focc_edge_faces(const FShape* s, int* out, int cap) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape edges, faces;
            edge_map(s->s, edges);
            face_map(s->s, faces);
            if (2 * edges.Extent() > cap) {
                set_err("edge_faces: buffer too small");
                return -1;
            }
            TopTools_IndexedDataMapOfShapeListOfShape adj;
            TopExp::MapShapesAndUniqueAncestors(s->s, TopAbs_EDGE, TopAbs_FACE, adj);
            for (int i = 1; i <= edges.Extent(); i++) {
                int fa = -1, fb = -1;
                const TopoDS_Shape& e = edges(i);
                if (adj.Contains(e)) {
                    for (TopTools_ListIteratorOfListOfShape it(adj.FindFromKey(e)); it.More(); it.Next()) {
                        int j = faces.FindIndex(it.Value()) - 1;
                        if (fa < 0) fa = j;
                        else if (fb < 0 && j != fa) fb = j;
                    }
                }
                out[2 * (i - 1)] = fa;
                out[2 * (i - 1) + 1] = fb;
            }
            return edges.Extent();
        },
        -1);
}

// --------------------------------------------------------- fillets, chamfers

// Rounds the edges (1-based indices) with radius r, all in one operation
// (corners where they meet are blended).
FShape* focc_fillet(const FShape* s, const int* idx, int n, double r, long long fresh) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            BRepFilletAPI_MakeFillet op(s->s);
            for (int i = 0; i < n; i++)
                if (idx[i] >= 1 && idx[i] <= m.Extent()) op.Add(r, TopoDS::Edge(m(idx[i])));
            op.Build();
            if (!op.IsDone()) {
                set_err("fillet failed");
                return (FShape*)nullptr;
            }
            if (!valid(op.Shape())) {
                set_err("fillet gave an invalid solid");
                return (FShape*)nullptr;
            }
            FShape* res = wrap(op.Shape());
            if (res) carry_tags(op, s, res, fresh);
            return res;
        },
        (FShape*)nullptr);
}

FShape* focc_chamfer(const FShape* s, const int* idx, int n, double d, long long fresh) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            BRepFilletAPI_MakeChamfer op(s->s);
            for (int i = 0; i < n; i++)
                if (idx[i] >= 1 && idx[i] <= m.Extent()) op.Add(d, TopoDS::Edge(m(idx[i])));
            op.Build();
            if (!op.IsDone()) {
                set_err("chamfer failed");
                return (FShape*)nullptr;
            }
            if (!valid(op.Shape())) {
                set_err("chamfer gave an invalid solid");
                return (FShape*)nullptr;
            }
            FShape* res = wrap(op.Shape());
            if (res) carry_tags(op, s, res, fresh);
            return res;
        },
        (FShape*)nullptr);
}

// ------------------------------------------------------------------- queries

double focc_volume(const FShape* s) {
    return guard(
        [&] {
            GProp_GProps p;
            BRepGProp::VolumeProperties(s->s, p);
            return p.Mass();
        },
        -1.0);
}

int focc_is_valid(const FShape* s) {
    return guard([&] { return valid(s->s) ? 1 : 0; }, -1);
}

// -------------------------------------------------------------------- mesh

// Triangulates the shape (chord deflection `defl` mm, angle `angle` rad):
// per-face vertices with their normals, triangles counter-clockwise outside.
FMesh* focc_mesh(const FShape* s, double defl, double angle) {
    return guard(
        [&] {
            BRepMesh_IncrementalMesh mesher(s->s, defl, Standard_False, angle, Standard_True);
            FMesh* out = new FMesh();
            for (TopExp_Explorer ex(s->s, TopAbs_FACE); ex.More(); ex.Next()) {
                TopoDS_Face f = TopoDS::Face(ex.Current());
                TopLoc_Location loc;
                opencascade::handle<Poly_Triangulation> t = BRep_Tool::Triangulation(f, loc);
                if (t.IsNull()) continue;
                if (!t->HasNormals()) BRepLib_ToolTriangulatedShape::ComputeNormals(f, t);
                const gp_Trsf tr = loc.Transformation();
                const bool rev = f.Orientation() == TopAbs_REVERSED;
                const uint32_t base = (uint32_t)(out->pos.size() / 3);
                for (int i = 1; i <= t->NbNodes(); i++) {
                    gp_Pnt p = t->Node(i).Transformed(tr);
                    gp_Dir d = t->Normal(i).Transformed(tr);
                    double k = rev ? -1.0 : 1.0;
                    out->pos.insert(out->pos.end(), {(float)p.X(), (float)p.Y(), (float)p.Z()});
                    out->nrm.insert(out->nrm.end(), {(float)(d.X() * k), (float)(d.Y() * k), (float)(d.Z() * k)});
                }
                for (int i = 1; i <= t->NbTriangles(); i++) {
                    int a, b, c;
                    t->Triangle(i).Get(a, b, c);
                    if (rev) std::swap(b, c);
                    out->tri.insert(out->tri.end(), {base + a - 1, base + b - 1, base + c - 1});
                }
            }
            return out;
        },
        (FMesh*)nullptr);
}

int focc_mesh_vertices(const FMesh* m) { return (int)(m->pos.size() / 3); }
int focc_mesh_triangles(const FMesh* m) { return (int)(m->tri.size() / 3); }
const float* focc_mesh_positions(const FMesh* m) { return m->pos.data(); }
const float* focc_mesh_normals(const FMesh* m) { return m->nrm.data(); }
const uint32_t* focc_mesh_indices(const FMesh* m) { return m->tri.data(); }
void focc_mesh_free(FMesh* m) { delete m; }

// ------------------------------------------------------------- edge lines

// Every edge of the shape a person would draw: the borders between faces,
// fillet boundaries included; seams (where a closed surface such as a
// cylinder wraps onto itself) and degenerated edges (a sphere's poles) are
// left out. Each one within `defl` mm and `angle` rad of the true curve.
FLines* focc_lines(const FShape* s, double defl, double angle) {
    return guard(
        [&]() -> FLines* {
            TopTools_IndexedDataMapOfShapeListOfShape faces;
            TopExp::MapShapesAndUniqueAncestors(s->s, TopAbs_EDGE, TopAbs_FACE, faces);
            FLines* out = new FLines();
            for (int i = 1; i <= faces.Extent(); i++) {
                const TopoDS_Edge& e = TopoDS::Edge(faces.FindKey(i));
                if (BRep_Tool::Degenerated(e)) continue;
                bool seam = false;
                for (TopTools_ListIteratorOfListOfShape it(faces.FindFromIndex(i)); it.More(); it.Next()) {
                    if (BRep_Tool::IsClosed(e, TopoDS::Face(it.Value()))) seam = true;
                }
                if (seam) continue;
                BRepAdaptor_Curve c(e);
                GCPnts_TangentialDeflection d(c, angle, defl, 2);
                int n = d.NbPoints();
                if (n < 2) continue;
                for (int k = 1; k <= n; k++) {
                    gp_Pnt p = d.Value(k);
                    out->xyz.insert(out->xyz.end(), {p.X(), p.Y(), p.Z()});
                }
                out->counts.push_back(n);
            }
            return out;
        },
        (FLines*)nullptr);
}

int focc_lines_count(const FLines* l) { return (int)l->counts.size(); }
const int* focc_lines_counts(const FLines* l) { return l->counts.data(); }
int focc_lines_points(const FLines* l) { return (int)(l->xyz.size() / 3); }
const double* focc_lines_xyz(const FLines* l) { return l->xyz.data(); }
void focc_lines_free(FLines* l) { delete l; }

// ------------------------------------------------------------------ tags

int focc_face_count(const FShape* s) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            face_map(s->s, m);
            return m.Extent();
        },
        -1);
}

// Face `index` (0-based, face map order): see face_info for `out` (13
// doubles). Returns its kind, -1 on error.
int focc_face_info(const FShape* s, int index, double* out) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            face_map(s->s, m);
            if (index < 0 || index >= m.Extent()) return -1;
            return face_info(TopoDS::Face(m(index + 1)), out);
        },
        -1);
}

void focc_set_tags(FShape* s, const long long* tags, int n) { s->tags.assign(tags, tags + (n > 0 ? n : 0)); }

long long focc_face_tag(const FShape* s, int index) { return index >= 0 && index < (int)s->tags.size() ? s->tags[index] : -1; }

// -------------------------------------------------------------- topology

struct FBuf {
    std::vector<uint8_t> b;
};

// Everything the app shows and picks, in one little-endian buffer:
//   u32 faces, each: i64 tag, i32 kind, f64 info[13] (see face_info)
//   u32 vertices, f32 xyz each, then f32 normals
//   u32 triangles, u32 abc each, then u32 face index (0-based) each
//   u32 edges, each: i32 index (1-based, the edge map's, for fillets),
//       i32 face a, i32 face b (0-based, -1 = none), i32 kind (0 line,
//       1 circle, 2 other), i32 seam (a closed surface's own seam),
//       u32 points, f64 xyz each
// Degenerated edges (a sphere's poles) are left out.
FBuf* focc_topo(const FShape* s, double defl, double angle) {
    return guard(
        [&]() -> FBuf* {
            BRepMesh_IncrementalMesh mesher(s->s, defl, Standard_False, angle, Standard_True);
            TopTools_IndexedMapOfShape faces;
            face_map(s->s, faces);
            Buf out;
            out.put<uint32_t>((uint32_t)faces.Extent());
            double info[13];
            for (int i = 1; i <= faces.Extent(); i++) {
                long long tag = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                int kind = face_info(TopoDS::Face(faces(i)), info);
                out.put<int64_t>(tag);
                out.put<int32_t>(kind);
                for (double d : info) out.put<double>(d);
            }
            // triangles, per face (sharp edges stay sharp)
            std::vector<float> pos, nrm;
            std::vector<uint32_t> tri, tri_face;
            for (int i = 1; i <= faces.Extent(); i++) {
                TopoDS_Face f = TopoDS::Face(faces(i));
                TopLoc_Location loc;
                opencascade::handle<Poly_Triangulation> t = BRep_Tool::Triangulation(f, loc);
                if (t.IsNull()) continue;
                if (!t->HasNormals()) BRepLib_ToolTriangulatedShape::ComputeNormals(f, t);
                const gp_Trsf tr = loc.Transformation();
                const bool rev = f.Orientation() == TopAbs_REVERSED;
                const uint32_t base = (uint32_t)(pos.size() / 3);
                for (int k = 1; k <= t->NbNodes(); k++) {
                    gp_Pnt p = t->Node(k).Transformed(tr);
                    gp_Dir d = t->Normal(k).Transformed(tr);
                    double sg = rev ? -1.0 : 1.0;
                    pos.insert(pos.end(), {(float)p.X(), (float)p.Y(), (float)p.Z()});
                    nrm.insert(nrm.end(), {(float)(d.X() * sg), (float)(d.Y() * sg), (float)(d.Z() * sg)});
                }
                for (int k = 1; k <= t->NbTriangles(); k++) {
                    int a, b, c;
                    t->Triangle(k).Get(a, b, c);
                    if (rev) std::swap(b, c);
                    tri.insert(tri.end(), {base + a - 1, base + b - 1, base + c - 1});
                    tri_face.push_back((uint32_t)(i - 1));
                }
            }
            out.put<uint32_t>((uint32_t)(pos.size() / 3));
            for (float v : pos) out.put<float>(v);
            for (float v : nrm) out.put<float>(v);
            out.put<uint32_t>((uint32_t)tri_face.size());
            for (uint32_t v : tri) out.put<uint32_t>(v);
            for (uint32_t v : tri_face) out.put<uint32_t>(v);
            // edges
            TopTools_IndexedMapOfShape edges;
            edge_map(s->s, edges);
            TopTools_IndexedDataMapOfShapeListOfShape adj;
            TopExp::MapShapesAndUniqueAncestors(s->s, TopAbs_EDGE, TopAbs_FACE, adj);
            Buf eb;
            uint32_t count = 0;
            for (int i = 1; i <= edges.Extent(); i++) {
                const TopoDS_Edge& e = TopoDS::Edge(edges(i));
                if (BRep_Tool::Degenerated(e)) continue;
                int fa = -1, fb = -1, seam = 0;
                if (adj.Contains(e)) {
                    for (TopTools_ListIteratorOfListOfShape it(adj.FindFromKey(e)); it.More(); it.Next()) {
                        int j = faces.FindIndex(it.Value()) - 1;
                        if (fa < 0) fa = j;
                        else if (fb < 0 && j != fa) fb = j;
                        if (BRep_Tool::IsClosed(e, TopoDS::Face(it.Value()))) seam = 1;
                    }
                }
                BRepAdaptor_Curve c(e);
                int kind = c.GetType() == GeomAbs_Line ? 0 : c.GetType() == GeomAbs_Circle ? 1 : 2;
                GCPnts_TangentialDeflection d(c, angle, defl, 2);
                int n = d.NbPoints();
                if (n < 2) continue;
                eb.put<int32_t>(i);
                eb.put<int32_t>(fa);
                eb.put<int32_t>(fb);
                eb.put<int32_t>(kind);
                eb.put<int32_t>(seam);
                eb.put<uint32_t>((uint32_t)n);
                for (int k = 1; k <= n; k++) {
                    gp_Pnt p = d.Value(k);
                    eb.put<double>(p.X());
                    eb.put<double>(p.Y());
                    eb.put<double>(p.Z());
                }
                count++;
            }
            out.put<uint32_t>(count);
            out.b.insert(out.b.end(), eb.b.begin(), eb.b.end());
            FBuf* r = new FBuf();
            r->b.swap(out.b);
            return r;
        },
        (FBuf*)nullptr);
}

int focc_buf_len(const FBuf* b) { return (int)b->b.size(); }
const uint8_t* focc_buf_data(const FBuf* b) { return b->b.data(); }
void focc_buf_free(FBuf* b) { delete b; }

}  // extern "C"
