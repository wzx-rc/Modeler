// Forme's thin C layer over OpenCascade (Romain, 17 nov. 2026: robust
// booleans and fillets, and a path to the iPad).
//
// Every entry point catches OpenCascade's exceptions and returns null (or a
// negative count) with the message in focc_last_error(), so a failed fillet
// never takes the app down. Only opencascade::handle<T> is used, never the
// Handle_T aliases (a derived class, not a typedef, under MSVC).

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Curve2d.hxx>
#include <BRepTools_WireExplorer.hxx>
#include <TopoDS_Iterator.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepLProp_SLProps.hxx>
#include <BRepTools.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepAlgoAPI_Splitter.hxx>
#include <BRepBndLib.hxx>
#include <BRepLib.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepOffsetAPI_MakeOffset.hxx>
#include <Bnd_Box.hxx>
#include <gp_Pln.hxx>
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
#include <BRep_Builder.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <Geom_BezierSurface.hxx>
#include <GC_MakeArcOfCircle.hxx>
#include <Geom2d_BSplineCurve.hxx>
#include <TColgp_Array1OfPnt2d.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <Precision.hxx>
#include <TColgp_Array2OfPnt.hxx>
#include <TopTools_DataMapOfShapeShape.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <ShapeAnalysis_Surface.hxx>
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
#include <gp_Pnt2d.hxx>
#include <gp_Trsf.hxx>

#include <BRepOffset_MakeOffset.hxx>
#include <BRepBuilderAPI_Sewing.hxx>
#include <gp_Circ.hxx>
#include <gp_Ax3.hxx>
#include <TopoDS_Compound.hxx>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <BRepCheck_Result.hxx>
#include <BRepCheck_ListOfStatus.hxx>
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
FShape* focc_prism_cut(const double* xy, const int* smooth, const int* cut, const double* abm, int n, double h);

FShape* focc_prism(const double* xy, const int* smooth, int n, double h) { return focc_prism_cut(xy, smooth, nullptr, nullptr, n, h); }

// The same, corners cut: cut[i] 1 a fillet (an arc through abm[6 i..]: its
// start, middle, end), 2 a chamfer (a straight side from start to end).
FShape* focc_prism_cut(const double* xy, const int* smooth, const int* cut, const double* abm, int n, double h) {
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
                int ck = cut && !round ? cut[i] : 0;
                if (ck) {
                    vin[i] = BRepBuilderAPI_MakeVertex(gp_Pnt(abm[6 * i], abm[6 * i + 1], -h));
                    vout[i] = BRepBuilderAPI_MakeVertex(gp_Pnt(abm[6 * i + 4], abm[6 * i + 5], -h));
                } else if (round) {
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
                int ck = cut ? cut[i] : 0;
                if (!vin[i].IsSame(vout[i]) && ck == 2) {
                    mw.Add(BRepBuilderAPI_MakeEdge(vin[i], vout[i]).Edge());
                } else if (!vin[i].IsSame(vout[i]) && ck == 1) {
                    GC_MakeArcOfCircle arc(BRep_Tool::Pnt(vin[i]), gp_Pnt(abm[6 * i + 2], abm[6 * i + 3], -h), BRep_Tool::Pnt(vout[i]));
                    if (!arc.IsDone()) {
                        set_err("a rounded corner could not be made");
                        return nullptr;
                    }
                    mw.Add(BRepBuilderAPI_MakeEdge(arc.Value(), vin[i], vout[i]).Edge());
                } else if (!vin[i].IsSame(vout[i])) {
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
    // a face made of faces of both (flush faces merged) keeps the name it
    // had in the part (a): the tool's tags first, the part's over them
    carry_tags(op, b, r, -1);
    carry_tags(op, a, r, -1);
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

// Rounds several fillets at once, each edge (1-based index) with its own
// radius and tag: where they meet, OpenCascade blends the corner instead of
// ending each fillet on its own. The faces made along an edge's contour take
// that edge's tag; the corner faces (made from vertices) take `fresh`.
FShape* focc_fillet_multi(const FShape* s, const int* idx, const double* r, const long long* tags, int n, long long fresh) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            BRepFilletAPI_MakeFillet op(s->s);
            std::vector<std::pair<TopoDS_Edge, long long>> added;
            for (int i = 0; i < n; i++) {
                if (idx[i] < 1 || idx[i] > m.Extent()) continue;
                TopoDS_Edge e = TopoDS::Edge(m(idx[i]));
                if (op.Contour(e) > 0) continue; // already in a contour (tangent chain)
                op.Add(r[i], e);
                added.push_back({e, tags[i]});
            }
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
            if (!res) return res;
            carry_tags(op, s, res, fresh);
            TopTools_IndexedMapOfShape fout;
            face_map(res->s, fout);
            for (auto& [e0, t] : added) {
                int ic = op.Contour(e0);
                if (ic <= 0) continue;
                for (int ie = 1; ie <= op.NbEdges(ic); ie++) {
                    const TopTools_ListOfShape& g = op.Generated(op.Edge(ic, ie));
                    for (TopTools_ListOfShape::Iterator it(g); it.More(); it.Next()) {
                        int j = fout.FindIndex(it.Value());
                        if (j > 0) res->tags[j - 1] = t;
                    }
                }
            }
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

// ------------------------------------------------------------------ insets

// The inside of a flat face, its outline moved in by d (its holes grow by
// d too), as a new face on the same plane, facing the same way. Sharp
// corners stay sharp. d = 0: a copy of the face.
FShape* focc_inset_face(const FShape* s, int index, double d) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            if (index < 0 || index >= fm.Extent()) {
                set_err("no such face");
                return nullptr;
            }
            TopoDS_Face f = TopoDS::Face(fm(index + 1));
            BRepAdaptor_Surface as(f);
            if (as.GetType() != GeomAbs_Plane) {
                set_err("an inset needs a flat face");
                return nullptr;
            }
            if (d < 0) {
                set_err("an inset needs a distance");
                return nullptr;
            }
            TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
            // the outline moved in (d = 0: the face's own wires)
            TopoDS_Shape ws = fw;
            if (d > 0) {
                BRepOffsetAPI_MakeOffset mo(fw, GeomAbs_Intersection);
                mo.Perform(-d);
                if (!mo.IsDone() || mo.Shape().IsNull()) {
                    set_err("the inset is wider than the face");
                    return nullptr;
                }
                // 3D curves for the offset edges (they come as curves on the face)
                ws = mo.Shape();
                BRepLib::BuildCurves3d(ws);
            }
            // the wires: the biggest is the outline, the others holes
            std::vector<TopoDS_Wire> wires;
            for (TopExp_Explorer x(ws, TopAbs_WIRE); x.More(); x.Next()) wires.push_back(TopoDS::Wire(x.Current()));
            if (wires.empty()) {
                set_err("the inset is wider than the face");
                return nullptr;
            }
            int outer = 0;
            double best = -1;
            for (size_t i = 0; i < wires.size(); i++) {
                Bnd_Box b;
                BRepBndLib::Add(wires[i], b);
                if (b.IsVoid()) continue;
                double size = b.SquareExtent();
                if (size > best) {
                    best = size;
                    outer = (int)i;
                }
            }
            gp_Pln pl = as.Plane();
            // each wire rebuilt from fresh edges on their 3D curves, so the
            // face owns clean edges (the offset's carry the old face's pcurves)
            auto rebuild = [](const TopoDS_Wire& w) -> TopoDS_Wire {
                BRepBuilderAPI_MakeWire mw;
                for (BRepTools_WireExplorer x(w); x.More(); x.Next()) {
                    TopoDS_Edge e = x.Current();
                    double a, b;
                    opencascade::handle<Geom_Curve> c = BRep_Tool::Curve(e, a, b);
                    if (c.IsNull()) return TopoDS_Wire();
                    TopoDS_Edge ne = BRepBuilderAPI_MakeEdge(c, a, b);
                    if (e.Orientation() == TopAbs_REVERSED) ne.Reverse();
                    mw.Add(ne);
                    if (!mw.IsDone()) return TopoDS_Wire();
                }
                return mw.IsDone() ? mw.Wire() : TopoDS_Wire();
            };
            auto area = [](const TopoDS_Face& x) {
                GProp_GProps g;
                BRepGProp::SurfaceProperties(x, g);
                return g.Mass();
            };
            // a wire as a face on the plane, wound so that its area is positive
            auto disc = [&](const TopoDS_Wire& w) -> TopoDS_Face {
                TopoDS_Face x = BRepBuilderAPI_MakeFace(pl, w, Standard_True);
                if (area(x) < 0) x = BRepBuilderAPI_MakeFace(pl, TopoDS::Wire(w.Reversed()), Standard_True);
                return x;
            };
            TopoDS_Wire ow = rebuild(wires[outer]);
            if (ow.IsNull()) {
                set_err("the inset outline is not closed");
                return nullptr;
            }
            TopoDS_Face r = disc(ow);
            if (area(r) <= 0) {
                set_err("the inset outline is not closed");
                return nullptr;
            }
            // holes added the other way round (their area counts negative)
            if (wires.size() > 1) {
                BRepBuilderAPI_MakeFace mf(r);
                for (size_t i = 0; i < wires.size(); i++) {
                    if ((int)i == outer) continue;
                    TopoDS_Wire hw = rebuild(wires[i]);
                    if (hw.IsNull()) {
                        set_err("a hole of the inset could not be made");
                        return nullptr;
                    }
                    TopoDS_Face h = disc(hw);
                    TopoDS_Wire hole = BRepTools::OuterWire(h);
                    mf.Add(TopoDS::Wire(hole.Reversed()));
                }
                if (!mf.IsDone()) {
                    set_err("the inset's holes could not be made");
                    return nullptr;
                }
                r = mf.Face();
                double full = area(r);
                if (full <= 0) {
                    set_err("the inset's holes fill it");
                    return nullptr;
                }
            }
            // facing like the face it comes from
            gp_Dir want = pl.Axis().Direction();
            if (f.Orientation() == TopAbs_REVERSED) want.Reverse();
            BRepAdaptor_Surface rs(r);
            gp_Dir got = rs.Plane().Axis().Direction();
            if (r.Orientation() == TopAbs_REVERSED) got.Reverse();
            if (got.Dot(want) < 0) r.Reverse();
            return wrap(r);
        },
        (FShape*)nullptr);
}

// A face swept along (dx, dy, dz) into a solid.
FShape* focc_extrude(const FShape* s, double dx, double dy, double dz) {
    return guard(
        [&]() -> FShape* {
            BRepPrimAPI_MakePrism op(s->s, gp_Vec(dx, dy, dz));
            if (!op.IsDone()) {
                set_err("extrusion failed");
                return nullptr;
            }
            TopoDS_Shape r = op.Shape();
            if (!valid(r)) {
                set_err("extrusion gave an invalid solid");
                return nullptr;
            }
            return wrap(r);
        },
        (FShape*)nullptr);
}

// The solid with the tool (a face on its boundary) drawn into its faces:
// the face under the tool is cut along the tool's outline, its inside
// takes the tool's tag.
FShape* focc_imprint(const FShape* s, const FShape* tool) {
    return guard(
        [&]() -> FShape* {
            BRepAlgoAPI_Splitter op;
            TopTools_ListOfShape args, tools;
            args.Append(s->s);
            tools.Append(tool->s);
            op.SetArguments(args);
            op.SetTools(tools);
            op.Build();
            if (!op.IsDone() || op.HasErrors()) {
                set_err("imprint failed");
                return nullptr;
            }
            // the solid alone (the splitter gives a compound)
            TopoDS_Shape r = op.Shape();
            int solids = 0;
            TopoDS_Shape one;
            for (TopExp_Explorer x(r, TopAbs_SOLID); x.More(); x.Next()) {
                solids++;
                one = x.Current();
            }
            if (solids == 1) r = one;
            FShape* res = wrap(r);
            if (!res) return nullptr;
            carry_tags(op, s, res, -1);
            carry_tags(op, tool, res, -1);
            return res;
        },
        (FShape*)nullptr);
}

// ------------------------------------------------------- component moves

// The points of the shape's vertices (vertex map order, 1-based index i at
// xyz[3 (i - 1)]); returns how many.
int focc_vertex_points(const FShape* s, double* xyz, int cap) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape vm;
            TopExp::MapShapes(s->s, TopAbs_VERTEX, vm);
            for (int i = 1; i <= vm.Extent() && i <= cap; i++) {
                gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(vm(i)));
                xyz[3 * (i - 1)] = p.X();
                xyz[3 * (i - 1) + 1] = p.Y();
                xyz[3 * (i - 1) + 2] = p.Z();
            }
            return vm.Extent();
        },
        -1);
}

// The vertices (1-based, vertex map order) of edge `index` (kind 1,
// 1-based edge map index) or face `index` (kind 2, 0-based face map index).
int focc_sub_vertices(const FShape* s, int kind, int index, int* out, int cap) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape vm, sub;
            TopExp::MapShapes(s->s, TopAbs_VERTEX, vm);
            TopoDS_Shape x;
            if (kind == 1) {
                edge_map(s->s, sub);
                if (index < 1 || index > sub.Extent()) return -1;
                x = sub(index);
            } else {
                face_map(s->s, sub);
                if (index < 0 || index >= sub.Extent()) return -1;
                x = sub(index + 1);
            }
            TopTools_IndexedMapOfShape mine;
            TopExp::MapShapes(x, TopAbs_VERTEX, mine);
            int n = 0;
            for (int i = 1; i <= mine.Extent(); i++) {
                int j = vm.FindIndex(mine(i));
                if (j > 0 && n < cap) out[n++] = j;
            }
            return n;
        },
        -1);
}

// The solid with some vertices moved (1-based indices, new points), direct
// modelling style: the faces round them follow. A face that stays flat
// stays a plane, a four-sided face that bends becomes a bilinear patch.
// For now the faces that follow must be flat with straight edges, their
// holes untouched. Tags stay on their faces.
FShape* focc_move_vertices(const FShape* s, const int* vidx, const double* xyz, int n) {
    return guard(
        [&]() -> FShape* {
            // work on a copy: new pcurves are added to shared edges
            BRepBuilderAPI_Copy cp(s->s);
            TopoDS_Shape src = cp.Shape();
            TopTools_IndexedMapOfShape vm, fm;
            TopExp::MapShapes(src, TopAbs_VERTEX, vm);
            face_map(src, fm);
            TopTools_DataMapOfShapeShape vnew, enew;
            for (int i = 0; i < n; i++) {
                if (vidx[i] < 1 || vidx[i] > vm.Extent()) {
                    set_err("no such vertex");
                    return nullptr;
                }
                TopoDS_Vertex nv = BRepBuilderAPI_MakeVertex(gp_Pnt(xyz[3 * i], xyz[3 * i + 1], xyz[3 * i + 2]));
                vnew.Bind(vm(vidx[i]), nv);
            }
            Bnd_Box box;
            BRepBndLib::Add(src, box);
            double size = box.IsVoid() ? 1.0 : std::sqrt(box.SquareExtent());
            BRep_Builder bb;
            TopoDS_Shell shell;
            bb.MakeShell(shell);
            std::vector<long long> tags;
            for (int i = 1; i <= fm.Extent(); i++) {
                TopoDS_Face f = TopoDS::Face(fm(i));
                long long tag = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                bool touched = false;
                for (TopExp_Explorer x(f, TopAbs_VERTEX); x.More() && !touched; x.Next()) touched = vnew.IsBound(x.Current());
                if (!touched) {
                    bb.Add(shell, f);
                    tags.push_back(tag);
                    continue;
                }
                BRepAdaptor_Surface as(f);
                if (as.GetType() != GeomAbs_Plane) {
                    set_err("only flat faces can follow a move for now");
                    return nullptr;
                }
                TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
                TopoDS_Wire ow = BRepTools::OuterWire(fw);
                std::vector<TopoDS_Wire> inner;
                for (TopExp_Explorer x(fw, TopAbs_WIRE); x.More(); x.Next())
                    if (!x.Current().IsSame(ow)) inner.push_back(TopoDS::Wire(x.Current()));
                for (const TopoDS_Wire& w : inner)
                    for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next())
                        if (vnew.IsBound(x.Current())) {
                            set_err("a hole or a boss next to the moved element cannot follow yet");
                            return nullptr;
                        }
                // the outline again, its moved edges remade (once, shared
                // with the face on their other side)
                BRepBuilderAPI_MakeWire mw;
                std::vector<gp_Pnt> pts;
                std::vector<TopoDS_Vertex> corners;
                for (BRepTools_WireExplorer x(ow, fw); x.More(); x.Next()) {
                    TopoDS_Edge e = x.Current();
                    TopoDS_Vertex cv = x.CurrentVertex();
                    pts.push_back(vnew.IsBound(cv) ? BRep_Tool::Pnt(TopoDS::Vertex(vnew(cv))) : BRep_Tool::Pnt(cv));
                    corners.push_back(vnew.IsBound(cv) ? TopoDS::Vertex(vnew(cv)) : cv);
                    TopoDS_Vertex a, b;
                    TopExp::Vertices(e, a, b);
                    bool ma = vnew.IsBound(a), mb = vnew.IsBound(b);
                    if (!ma && !mb) {
                        mw.Add(e);
                        continue;
                    }
                    BRepAdaptor_Curve c(e);
                    if (c.GetType() != GeomAbs_Line) {
                        set_err("only straight edges can follow a move for now");
                        return nullptr;
                    }
                    TopoDS_Edge ne;
                    if (enew.IsBound(e)) {
                        ne = TopoDS::Edge(enew(e));
                    } else {
                        TopoDS_Vertex na = ma ? TopoDS::Vertex(vnew(a)) : a;
                        TopoDS_Vertex nb = mb ? TopoDS::Vertex(vnew(b)) : b;
                        if (BRep_Tool::Pnt(na).Distance(BRep_Tool::Pnt(nb)) < 1e-4) {
                            set_err("the move folds an edge to nothing");
                            return nullptr;
                        }
                        BRepBuilderAPI_MakeEdge me(TopoDS::Vertex(na.Oriented(TopAbs_FORWARD)), TopoDS::Vertex(nb.Oriented(TopAbs_FORWARD)));
                        if (!me.IsDone()) {
                            set_err("an edge could not follow the move");
                            return nullptr;
                        }
                        ne = me.Edge();
                        enew.Bind(e.Oriented(TopAbs_FORWARD), ne);
                    }
                    // same direction as the old edge: same orientation in the wire
                    mw.Add(TopoDS::Edge(ne.Oriented(e.Orientation())));
                }
                if (!mw.IsDone()) {
                    set_err("a face's outline could not follow the move");
                    return nullptr;
                }
                TopoDS_Wire nw = mw.Wire();
                // flat still? (Newell's normal, along the surface's own
                // normal for an outline wound counter-clockwise)
                gp_XYZ nrm(0, 0, 0), cen(0, 0, 0);
                for (size_t k = 0; k < pts.size(); k++) {
                    const gp_Pnt& p = pts[k];
                    const gp_Pnt& q = pts[(k + 1) % pts.size()];
                    nrm += gp_XYZ((p.Y() - q.Y()) * (p.Z() + q.Z()), (p.Z() - q.Z()) * (p.X() + q.X()), (p.X() - q.X()) * (p.Y() + q.Y()));
                    cen += p.XYZ();
                }
                if (pts.size() < 3 || nrm.Modulus() < 1e-12) {
                    set_err("the move folds a face flat");
                    return nullptr;
                }
                cen /= (double)pts.size();
                gp_Dir nd(nrm);
                double off = 0;
                for (const gp_Pnt& p : pts) off = std::max(off, std::abs((p.XYZ() - cen).Dot(nd.XYZ())));
                TopoDS_Face nf;
                if (off <= 1e-6 * size + 1e-6) {
                    gp_Pln pl(gp_Pnt(cen), nd);
                    gp_Pln old = as.Plane();
                    bool same = std::abs(std::abs(old.Axis().Direction().Dot(nd)) - 1) < 1e-9 && old.Distance(gp_Pnt(cen)) < 1e-6 * size + 1e-7;
                    if (!inner.empty() && !same) {
                        set_err("a face with a hole cannot tilt or move off its plane yet");
                        return nullptr;
                    }
                    if (same) pl = gp_Pln(old.Location(), old.Axis().Direction().Dot(nd) > 0 ? old.Axis().Direction() : old.Axis().Direction().Reversed());
                    BRepBuilderAPI_MakeFace mf(pl, nw, Standard_True);
                    for (const TopoDS_Wire& w : inner) mf.Add(w);
                    if (!mf.IsDone()) {
                        set_err("a face could not follow the move");
                        return nullptr;
                    }
                    nf = mf.Face();
                } else if (pts.size() == 4 && inner.empty()) {
                    // a bent four-sided face: the bilinear patch on its corners
                    TColgp_Array2OfPnt poles(1, 2, 1, 2);
                    poles(1, 1) = pts[0];
                    poles(2, 1) = pts[1];
                    poles(2, 2) = pts[2];
                    poles(1, 2) = pts[3];
                    opencascade::handle<Geom_BezierSurface> bs = new Geom_BezierSurface(poles);
                    // each edge's curve on the patch: a straight segment
                    // between its corners' (u, v), linear in the edge's
                    // parameter like the edge itself
                    const double cuv[4][2] = {{0, 0}, {1, 0}, {1, 1}, {0, 1}};
                    bb.MakeFace(nf, bs, Precision::Confusion());
                    bb.Add(nf, nw);
                    for (TopExp_Explorer x(nw, TopAbs_EDGE); x.More(); x.Next()) {
                        TopoDS_Edge e = TopoDS::Edge(x.Current());
                        TopoDS_Vertex a, b;
                        TopExp::Vertices(e, a, b);
                        int ia = -1, ib = -1;
                        for (int k = 0; k < 4; k++) {
                            if (corners[k].IsSame(a)) ia = k;
                            if (corners[k].IsSame(b)) ib = k;
                        }
                        double t0, t1;
                        BRep_Tool::Range(e, t0, t1);
                        if (ia < 0 || ib < 0 || t1 <= t0) {
                            set_err("a bent face could not follow the move");
                            return nullptr;
                        }
                        TColgp_Array1OfPnt2d pp(1, 2);
                        pp(1) = gp_Pnt2d(cuv[ia][0], cuv[ia][1]);
                        pp(2) = gp_Pnt2d(cuv[ib][0], cuv[ib][1]);
                        TColStd_Array1OfReal kn(1, 2);
                        kn(1) = t0;
                        kn(2) = t1;
                        TColStd_Array1OfInteger mu(1, 2);
                        mu(1) = 2;
                        mu(2) = 2;
                        opencascade::handle<Geom2d_BSplineCurve> pc = new Geom2d_BSplineCurve(pp, kn, mu, 1);
                        bb.UpdateEdge(e, pc, nf, BRep_Tool::Tolerance(e));
                    }
                } else if (!inner.empty()) {
                    set_err("a face with a hole cannot bend yet");
                    return nullptr;
                } else {
                    set_err("a face with more than four sides would bend; move it along its plane");
                    return nullptr;
                }
                nf.Orientation(f.Orientation());
                bb.Add(shell, nf);
                tags.push_back(tag);
            }
            shell.Closed(BRep_Tool::IsClosed(shell));
            TopoDS_Solid solid;
            bb.MakeSolid(solid);
            bb.Add(solid, shell);
            BRepLib::OrientClosedSolid(solid);
            if (!valid(solid)) {
                if (getenv("FORME_OCCT_DEBUG")) {
                    BRepCheck_Analyzer an(solid);
                    for (TopAbs_ShapeEnum t : {TopAbs_VERTEX, TopAbs_EDGE, TopAbs_WIRE, TopAbs_FACE, TopAbs_SHELL, TopAbs_SOLID})
                        for (TopExp_Explorer x(solid, t); x.More(); x.Next()) {
                            opencascade::handle<BRepCheck_Result> res = an.Result(x.Current());
                            if (res.IsNull()) continue;
                            for (BRepCheck_ListOfStatus::Iterator it(res->Status()); it.More(); it.Next())
                                if (it.Value() != BRepCheck_NoError) fprintf(stderr, "check: shape type %d status %d\n", (int)t, (int)it.Value());
                            for (res->InitContextIterator(); res->MoreShapeInContext(); res->NextShapeInContext())
                                for (BRepCheck_ListOfStatus::Iterator it(res->StatusOnShape()); it.More(); it.Next())
                                    if (it.Value() != BRepCheck_NoError) fprintf(stderr, "check: shape type %d in context status %d\n", (int)t, (int)it.Value());
                        }
                }
                set_err("the move gives an invalid solid (a face crosses another?)");
                return nullptr;
            }
            FShape* r = wrap(solid);
            if (!r) return nullptr;
            r->tags = tags;
            return r;
        },
        (FShape*)nullptr);
}

// Distance from a point to a face (-1 when it cannot be measured).
double focc_face_distance(const FShape* s, int index, const double* p) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            if (index < 0 || index >= fm.Extent()) return -1.0;
            TopoDS_Vertex v = BRepBuilderAPI_MakeVertex(gp_Pnt(p[0], p[1], p[2]));
            BRepExtrema_DistShapeShape dss(v, fm(index + 1));
            if (!dss.IsDone()) return -1.0;
            return dss.Value();
        },
        -1.0);
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

// ------------------------------------------------- Forme Rectify's shapes
//
// A shape drawn on a scan (rectify-app `body`): a flat outline (a face, or
// a line of free edges), an edge pulled into a new face, the faces
// thickened into a solid, edges filleted.

// A tag made from two numbers (a thickness's top and side faces): the same
// numbers give the same tag; always positive, never one of the app's step
// tags (bit 40 set).
static long long mix_tag(long long a, long long b) {
    unsigned long long x = (unsigned long long)a * 0x9E3779B97F4A7C15ULL;
    x ^= (unsigned long long)b + 0x632BE59BD9B4E019ULL + (x << 6) + (x >> 2);
    x ^= x >> 31;
    x *= 0xBF58476D1CE4E5B9ULL;
    x ^= x >> 29;
    return (long long)(x & 0x3FFFFFFFFFFFFFFFULL) | (1LL << 40);
}

// The faces of a shape, and its free edges (on no face: a line's).
static void split_free(const TopoDS_Shape& s, std::vector<TopoDS_Face>& faces, std::vector<TopoDS_Edge>& free_edges) {
    TopTools_IndexedMapOfShape fm, em;
    face_map(s, fm);
    for (int i = 1; i <= fm.Extent(); i++) faces.push_back(TopoDS::Face(fm(i)));
    TopTools_IndexedDataMapOfShapeListOfShape adj;
    TopExp::MapShapesAndAncestors(s, TopAbs_EDGE, TopAbs_FACE, adj);
    edge_map(s, em);
    for (int i = 1; i <= em.Extent(); i++) {
        const TopoDS_Shape& e = em(i);
        int k = adj.FindIndex(e);
        if (k == 0 || adj(k).IsEmpty()) free_edges.push_back(TopoDS::Edge(e));
    }
}

// The part of a shape made of faces: its solid, its shell, its face, or a
// compound of its faces (null: none).
static TopoDS_Shape faces_part(const TopoDS_Shape& s) {
    TopoDS_Shape found[3];
    int count[3] = {0, 0, 0};
    const TopAbs_ShapeEnum kinds[3] = {TopAbs_SOLID, TopAbs_SHELL, TopAbs_FACE};
    for (int k = 0; k < 3; k++)
        for (TopExp_Explorer x(s, kinds[k]); x.More(); x.Next()) {
            if (!count[k]) found[k] = x.Current();
            count[k]++;
        }
    for (int k = 0; k < 3; k++)
        if (count[k] == 1) return found[k];
    if (!count[2]) return TopoDS_Shape();
    BRep_Builder bb;
    TopoDS_Compound c;
    bb.MakeCompound(c);
    for (TopExp_Explorer x(s, TopAbs_FACE); x.More(); x.Next()) bb.Add(c, x.Current());
    return c;
}

// A shape with free edges added (a compound when there are some).
static TopoDS_Shape with_free(const TopoDS_Shape& body, const std::vector<TopoDS_Edge>& free_edges) {
    if (free_edges.empty()) return body;
    BRep_Builder bb;
    TopoDS_Compound c;
    bb.MakeCompound(c);
    if (!body.IsNull()) bb.Add(c, body);
    for (const TopoDS_Edge& e : free_edges) bb.Add(c, e);
    return c;
}

// The outward normal of a flat face.
static gp_Dir plane_normal(const TopoDS_Face& f) {
    BRepAdaptor_Surface sa(f);
    gp_Dir n = sa.Plane().Axis().Direction();
    if (f.Orientation() == TopAbs_REVERSED) n.Reverse();
    return n;
}

// A flat outline through n points (xyz, on the plane through o facing
// nrm): closed, a face facing nrm; open, a line (its edges free). Corner i
// cut: cut[i] 1, an arc through abm[9 i..9 i + 9] (its start, middle and
// end), 2, a straight side from start to end. An open line's ends are never
// cut.
FShape* focc_outline(const double* o, const double* nrm, const double* xyz, int n, int closed, const int* cut, const double* abm) {
    return guard(
        [&]() -> FShape* {
            if (n < 2 || (closed && n < 3)) {
                set_err("an outline needs more points");
                return nullptr;
            }
            auto P = [](const double* a) { return gp_Pnt(a[0], a[1], a[2]); };
            std::vector<TopoDS_Vertex> vin(n), vout(n);
            std::vector<int> ck(n, 0);
            for (int i = 0; i < n; i++) {
                bool end = !closed && (i == 0 || i == n - 1);
                ck[i] = (cut && !end) ? cut[i] : 0;
                if (ck[i]) {
                    vin[i] = BRepBuilderAPI_MakeVertex(P(abm + 9 * i));
                    vout[i] = BRepBuilderAPI_MakeVertex(P(abm + 9 * i + 6));
                } else {
                    vin[i] = BRepBuilderAPI_MakeVertex(P(xyz + 3 * i));
                    vout[i] = vin[i];
                }
            }
            BRepBuilderAPI_MakeWire mw;
            int edges = 0;
            for (int i = 0; i < n; i++) {
                if (ck[i] == 1) {
                    GC_MakeArcOfCircle arc(BRep_Tool::Pnt(vin[i]), P(abm + 9 * i + 3), BRep_Tool::Pnt(vout[i]));
                    if (!arc.IsDone()) {
                        set_err("a rounded corner could not be made");
                        return nullptr;
                    }
                    mw.Add(BRepBuilderAPI_MakeEdge(arc.Value(), vin[i], vout[i]).Edge());
                    edges++;
                } else if (ck[i] == 2) {
                    mw.Add(BRepBuilderAPI_MakeEdge(vin[i], vout[i]).Edge());
                    edges++;
                }
                if (closed || i + 1 < n) {
                    int j = (i + 1) % n;
                    if (BRep_Tool::Pnt(vout[i]).Distance(BRep_Tool::Pnt(vin[j])) > 1e-7) {
                        mw.Add(BRepBuilderAPI_MakeEdge(vout[i], vin[j]).Edge());
                        edges++;
                    }
                }
                if (!mw.IsDone() && edges > 0) {
                    set_err("the outline's sides do not join");
                    return nullptr;
                }
            }
            if (!edges || !mw.IsDone()) {
                set_err("the outline has no length");
                return nullptr;
            }
            TopoDS_Wire w = mw.Wire();
            if (!closed) return wrap(w);
            gp_Dir dn(nrm[0], nrm[1], nrm[2]);
            BRepBuilderAPI_MakeFace mf(gp_Pln(P(o), dn), w, Standard_True);
            if (!mf.IsDone()) {
                set_err("the outline does not make a face");
                return nullptr;
            }
            TopoDS_Face f = mf.Face();
            if (!valid(f)) {
                set_err("the outline crosses itself");
                return nullptr;
            }
            if (plane_normal(f).Dot(dn) < 0) f.Reverse();
            return wrap(f);
        },
        (FShape*)nullptr);
}

// A disc of radius r round o, facing nrm.
FShape* focc_disc(const double* o, const double* nrm, double r) {
    return guard(
        [&]() -> FShape* {
            if (r <= 1e-9) {
                set_err("the circle has no size");
                return nullptr;
            }
            gp_Dir dn(nrm[0], nrm[1], nrm[2]);
            gp_Ax2 ax(gp_Pnt(o[0], o[1], o[2]), dn);
            TopoDS_Edge e = BRepBuilderAPI_MakeEdge(gp_Circ(ax, r)).Edge();
            TopoDS_Wire w = BRepBuilderAPI_MakeWire(e).Wire();
            BRepBuilderAPI_MakeFace mf(gp_Pln(gp_Ax3(ax)), w, Standard_True);
            if (!mf.IsDone()) {
                set_err("the disc could not be made");
                return nullptr;
            }
            TopoDS_Face f = mf.Face();
            if (plane_normal(f).Dot(dn) < 0) f.Reverse();
            return wrap(f);
        },
        (FShape*)nullptr);
}

// Edge `index` (1-based) pulled along v into a new face (tag `tag`), sewn
// to the faces along it. A free edge (a line's) becomes the new face's
// side; the other free edges stay free. Faces keep their tags.
FShape* focc_extrude_edge(const FShape* s, int index, const double* v, long long tag) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape em;
            edge_map(s->s, em);
            if (index < 1 || index > em.Extent()) {
                set_err("no such edge");
                return nullptr;
            }
            gp_Vec vec(v[0], v[1], v[2]);
            if (vec.Magnitude() < 1e-6) {
                set_err("pulled too little");
                return nullptr;
            }
            TopoDS_Edge e = TopoDS::Edge(em(index));
            BRepPrimAPI_MakePrism pr(e, vec);
            pr.Build();
            if (!pr.IsDone()) {
                set_err("the edge could not be pulled");
                return nullptr;
            }
            TopoDS_Shape nf;
            for (TopExp_Explorer x(pr.Shape(), TopAbs_FACE); x.More() && nf.IsNull(); x.Next()) nf = x.Current();
            if (nf.IsNull()) {
                set_err("the edge made no face");
                return nullptr;
            }
            std::vector<TopoDS_Face> faces;
            std::vector<TopoDS_Edge> loose;
            split_free(s->s, faces, loose);
            loose.erase(std::remove_if(loose.begin(), loose.end(), [&](const TopoDS_Edge& x) { return x.IsSame(e); }), loose.end());
            BRepBuilderAPI_Sewing sew(1e-6);
            for (const TopoDS_Face& f : faces) sew.Add(f);
            sew.Add(nf);
            sew.Perform();
            TopoDS_Shape sewn = sew.SewedShape();
            if (sewn.IsNull()) {
                set_err("the new face could not be joined");
                return nullptr;
            }
            FShape* r = wrap(with_free(sewn, loose));
            if (!r) return r;
            TopTools_IndexedMapOfShape fin, fout;
            face_map(s->s, fin);
            face_map(r->s, fout);
            r->tags.assign(fout.Extent(), -1);
            auto image = [&](const TopoDS_Shape& f) -> int {
                if (sew.IsModified(f)) {
                    int j = fout.FindIndex(sew.Modified(f));
                    if (j > 0) return j;
                }
                return fout.FindIndex(f);
            };
            for (int i = 1; i <= fin.Extent(); i++) {
                int j = image(fin(i));
                if (j > 0) r->tags[j - 1] = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
            }
            int j = image(nf);
            if (j > 0) r->tags[j - 1] = tag;
            for (long long& t : r->tags)
                if (t < 0) t = tag;
            return r;
        },
        (FShape*)nullptr);
}

// The faces made a solid t thick along their normals (t < 0: the other
// way). One flat face: swept along its normal; else offset, the sides
// joined (OpenCascade's thickening). A face tagged T keeps its tag; its top
// gets mix(T, 8 salt + 1), its sides mix(mix(T, 8 salt + 2), k). Free edges
// stay as they are.
FShape* focc_thicken(const FShape* s, double t, long long salt) {
    return guard(
        [&]() -> FShape* {
            std::vector<TopoDS_Face> faces;
            std::vector<TopoDS_Edge> loose;
            split_free(s->s, faces, loose);
            if (faces.empty()) {
                set_err("nothing to thicken: close the outline, or pull an edge out first");
                return nullptr;
            }
            if (std::fabs(t) < 1e-6) {
                set_err("no thickness");
                return nullptr;
            }
            TopTools_IndexedMapOfShape fin;
            face_map(s->s, fin);
            auto tag_in = [&](int i) { return i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1LL; };
            if (faces.size() == 1 && BRepAdaptor_Surface(faces[0]).GetType() == GeomAbs_Plane) {
                const TopoDS_Face& f = faces[0];
                BRepPrimAPI_MakePrism pr(f, gp_Vec(plane_normal(f)) * t);
                pr.Build();
                if (!pr.IsDone()) {
                    set_err("the face could not be thickened");
                    return nullptr;
                }
                FShape* r = wrap(with_free(pr.Shape(), loose));
                if (!r) return r;
                TopTools_IndexedMapOfShape fout;
                face_map(r->s, fout);
                r->tags.assign(fout.Extent(), -1);
                auto put = [&](const TopoDS_Shape& x, long long tg) {
                    for (TopExp_Explorer ex(x, TopAbs_FACE); ex.More(); ex.Next()) {
                        int j = fout.FindIndex(ex.Current());
                        if (j > 0) r->tags[j - 1] = tg;
                    }
                };
                long long T = tag_in(fin.FindIndex(f));
                put(pr.FirstShape(), T);
                put(pr.LastShape(), mix_tag(T, 8 * salt + 1));
                TopTools_IndexedMapOfShape fe;
                edge_map(f, fe);
                long long side = mix_tag(T, 8 * salt + 2);
                for (int k = 1; k <= fe.Extent(); k++) {
                    const TopTools_ListOfShape& g = pr.Generated(fe(k));
                    for (TopTools_ListOfShape::Iterator it(g); it.More(); it.Next()) put(it.Value(), mix_tag(side, k));
                }
                for (long long& tg : r->tags)
                    if (tg < 0) tg = mix_tag(T, 8 * salt + 3);
                return r;
            }
            TopoDS_Shape in = faces_part(s->s);
            BRepOffset_MakeOffset mo;
            mo.Initialize(in, t, 1e-5, BRepOffset_Skin, Standard_False, Standard_False, GeomAbs_Intersection, Standard_True, Standard_False);
            mo.MakeOffsetShape();
            if (!mo.IsDone() || mo.Shape().IsNull()) {
                set_err("the faces could not be thickened");
                return nullptr;
            }
            FShape* r = wrap(with_free(mo.Shape(), loose));
            if (!r) return r;
            if (!valid(mo.Shape())) {
                delete r;
                set_err("the thickness gave an invalid solid");
                return nullptr;
            }
            // tags by where each face lies: on a face (it), at the
            // thickness from it (its top), else a side
            TopTools_IndexedMapOfShape fout;
            face_map(r->s, fout);
            r->tags.assign(fout.Extent(), -1);
            std::vector<int> sides(fin.Extent() + 1, 0);
            double info[13];
            for (int j = 1; j <= fout.Extent(); j++) {
                face_info(TopoDS::Face(fout(j)), info);
                TopoDS_Vertex mv = BRepBuilderAPI_MakeVertex(gp_Pnt(info[0], info[1], info[2]));
                int best = 0;
                double bd = 1e300;
                for (int i = 1; i <= fin.Extent(); i++) {
                    BRepExtrema_DistShapeShape d(mv, fin(i));
                    if (d.IsDone() && d.NbSolution() > 0 && d.Value() < bd) {
                        bd = d.Value();
                        best = i;
                    }
                }
                if (!best) continue;
                long long T = tag_in(best);
                double at = std::fabs(t);
                if (bd < 1e-4)
                    r->tags[j - 1] = T;
                else if (std::fabs(bd - at) < 1e-3 * std::max(1.0, at))
                    r->tags[j - 1] = mix_tag(T, 8 * salt + 1);
                else
                    r->tags[j - 1] = mix_tag(mix_tag(T, 8 * salt + 2), ++sides[best]);
            }
            for (long long& tg : r->tags)
                if (tg < 0) tg = mix_tag(salt, 8 * salt + 3);
            return r;
        },
        (FShape*)nullptr);
}

// Edges (1-based indices) rounded with radius r, in one operation, on the
// shape's faces (a solid or a sheet); free edges stay. The new faces get
// `fresh`, the others keep their tags.
FShape* focc_round(const FShape* s, const int* idx, int n, double r, long long fresh) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            std::vector<TopoDS_Face> faces;
            std::vector<TopoDS_Edge> loose;
            split_free(s->s, faces, loose);
            TopoDS_Shape part = faces_part(s->s);
            if (part.IsNull()) {
                set_err("a fillet needs faces");
                return nullptr;
            }
            BRepFilletAPI_MakeFillet op(part);
            int added = 0;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 1 && idx[i] <= m.Extent()) {
                    op.Add(r, TopoDS::Edge(m(idx[i])));
                    added++;
                }
            if (!added) {
                set_err("no edge to round");
                return nullptr;
            }
            op.Build();
            if (!op.IsDone()) {
                set_err("fillet failed");
                return nullptr;
            }
            if (!valid(op.Shape())) {
                set_err("fillet gave an invalid shape");
                return nullptr;
            }
            FShape* res = wrap(with_free(op.Shape(), loose));
            if (res) carry_tags(op, s, res, fresh);
            return res;
        },
        (FShape*)nullptr);
}

// Edges cut flat at distance d on each face (a bevel), on the shape's faces
// (a solid or a sheet), free edges kept; the new faces get `fresh`.
FShape* focc_bevel(const FShape* s, const int* idx, int n, double d, long long fresh) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            std::vector<TopoDS_Face> faces;
            std::vector<TopoDS_Edge> loose;
            split_free(s->s, faces, loose);
            TopoDS_Shape part = faces_part(s->s);
            if (part.IsNull()) {
                set_err("a bevel needs faces");
                return nullptr;
            }
            BRepFilletAPI_MakeChamfer op(part);
            int added = 0;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 1 && idx[i] <= m.Extent()) {
                    op.Add(d, TopoDS::Edge(m(idx[i])));
                    added++;
                }
            if (!added) {
                set_err("no edge to bevel");
                return nullptr;
            }
            op.Build();
            if (!op.IsDone()) {
                set_err("bevel failed");
                return nullptr;
            }
            if (!valid(op.Shape())) {
                set_err("bevel gave an invalid shape");
                return nullptr;
            }
            FShape* res = wrap(with_free(op.Shape(), loose));
            if (res) carry_tags(op, s, res, fresh);
            return res;
        },
        (FShape*)nullptr);
}

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

// ------------------------------------------------------------ quad mesh

// What the quad mesher (nurbs-kernel `quads.rs`) walks, in one buffer:
//   u32 faces, each: i32 kind (as face_info), u32 wires (the outer one
//       first), each: u32 n, each: i32 edge (1-based edge map), i32 reversed
//   u32 edges (edge map order), each: f64 t0, t1, i32 v0, v1 (vertex map,
//       1-based: the vertices at t0 and at t1), i32 kind (0 line, 1 circle,
//       2 other), f64 radius, i32 degenerated, 33 points f64 xyz from t0 to t1
FBuf* focc_quad_topo(const FShape* s) {
    return guard(
        [&]() -> FBuf* {
            TopTools_IndexedMapOfShape faces, edges, verts;
            face_map(s->s, faces);
            edge_map(s->s, edges);
            TopExp::MapShapes(s->s, TopAbs_VERTEX, verts);
            Buf out;
            out.put<uint32_t>((uint32_t)faces.Extent());
            double info[13];
            for (int i = 1; i <= faces.Extent(); i++) {
                TopoDS_Face f = TopoDS::Face(faces(i));
                out.put<int32_t>(face_info(f, info));
                TopoDS_Wire outer = BRepTools::OuterWire(f);
                std::vector<TopoDS_Wire> wires;
                if (!outer.IsNull()) wires.push_back(outer);
                for (TopoDS_Iterator it(f); it.More(); it.Next()) {
                    if (it.Value().ShapeType() == TopAbs_WIRE && !it.Value().IsSame(outer)) wires.push_back(TopoDS::Wire(it.Value()));
                }
                out.put<uint32_t>((uint32_t)wires.size());
                for (const TopoDS_Wire& w : wires) {
                    std::vector<std::pair<int, int>> list;
                    for (BRepTools_WireExplorer ex(w, f); ex.More(); ex.Next()) {
                        list.push_back({edges.FindIndex(ex.Current()), ex.Orientation() == TopAbs_REVERSED ? 1 : 0});
                    }
                    out.put<uint32_t>((uint32_t)list.size());
                    for (auto& e : list) {
                        out.put<int32_t>(e.first);
                        out.put<int32_t>(e.second);
                    }
                }
            }
            out.put<uint32_t>((uint32_t)edges.Extent());
            for (int i = 1; i <= edges.Extent(); i++) {
                TopoDS_Edge e = TopoDS::Edge(edges(i).Oriented(TopAbs_FORWARD));
                bool degen = BRep_Tool::Degenerated(e);
                double t0 = 0, t1 = 0;
                BRep_Tool::Range(e, t0, t1);
                TopoDS_Vertex a, b;
                TopExp::Vertices(e, a, b);
                out.put<double>(t0);
                out.put<double>(t1);
                out.put<int32_t>(a.IsNull() ? 0 : verts.FindIndex(a));
                out.put<int32_t>(b.IsNull() ? 0 : verts.FindIndex(b));
                int kind = 2;
                double r = 0;
                gp_Pnt pts[33];
                if (!degen) {
                    BRepAdaptor_Curve c(e);
                    if (c.GetType() == GeomAbs_Line) kind = 0;
                    if (c.GetType() == GeomAbs_Circle) {
                        kind = 1;
                        r = c.Circle().Radius();
                    }
                    for (int k = 0; k <= 32; k++) pts[k] = c.Value(t0 + (t1 - t0) * k / 32.0);
                } else if (!a.IsNull()) {
                    for (int k = 0; k <= 32; k++) pts[k] = BRep_Tool::Pnt(a);
                }
                out.put<int32_t>(kind);
                out.put<double>(r);
                out.put<int32_t>(degen ? 1 : 0);
                for (int k = 0; k <= 32; k++) {
                    out.put<double>(pts[k].X());
                    out.put<double>(pts[k].Y());
                    out.put<double>(pts[k].Z());
                }
            }
            FBuf* r = new FBuf();
            r->b.swap(out.b);
            return r;
        },
        (FBuf*)nullptr);
}

// Points of edge `edge` (1-based) at n parameters: 3D (xyz) and, on face
// `face` (0-based, the edge as it is oriented in that face: a seam has two
// curves there), its surface coordinates (uv). Returns n, or -1.
int focc_edge_on_face(const FShape* s, int face, int edge, int reversed, int n, const double* t, double* xyz, double* uv) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape faces, edges;
            face_map(s->s, faces);
            edge_map(s->s, edges);
            if (face < 0 || face >= faces.Extent() || edge < 1 || edge > edges.Extent()) {
                set_err("no such face or edge");
                return -1;
            }
            TopoDS_Face f = TopoDS::Face(faces(face + 1));
            TopoDS_Edge want = TopoDS::Edge(edges(edge));
            TopAbs_Orientation o = reversed ? TopAbs_REVERSED : TopAbs_FORWARD;
            TopoDS_Edge found;
            for (TopExp_Explorer ex(f, TopAbs_EDGE); ex.More(); ex.Next()) {
                if (ex.Current().IsSame(want) && (found.IsNull() || ex.Current().Orientation() == o)) found = TopoDS::Edge(ex.Current());
            }
            if (found.IsNull()) {
                set_err("the edge is not on that face");
                return -1;
            }
            BRepAdaptor_Curve c(found);
            BRepAdaptor_Curve2d c2(found, f);
            for (int k = 0; k < n; k++) {
                gp_Pnt p = c.Value(t[k]);
                gp_Pnt2d q = c2.Value(t[k]);
                xyz[3 * k] = p.X();
                xyz[3 * k + 1] = p.Y();
                xyz[3 * k + 2] = p.Z();
                uv[2 * k] = q.X();
                uv[2 * k + 1] = q.Y();
            }
            return n;
        },
        -1);
}

// Points of face `face` (0-based) at n surface coordinates: xyz and the
// outward normal (6 doubles each). Returns n, or -1.
int focc_face_eval(const FShape* s, int face, int n, const double* uv, double* out) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape faces;
            face_map(s->s, faces);
            if (face < 0 || face >= faces.Extent()) {
                set_err("no such face");
                return -1;
            }
            TopoDS_Face f = TopoDS::Face(faces(face + 1));
            BRepAdaptor_Surface sa(f, Standard_True);
            bool rev = f.Orientation() == TopAbs_REVERSED;
            for (int k = 0; k < n; k++) {
                double u = uv[2 * k], v = uv[2 * k + 1];
                gp_Pnt p = sa.Value(u, v);
                BRepLProp_SLProps props(sa, u, v, 1, 1e-7);
                gp_Dir d(0, 0, 1);
                if (props.IsNormalDefined()) d = props.Normal();
                if (rev) d.Reverse();
                double* o = out + 6 * k;
                o[0] = p.X(); o[1] = p.Y(); o[2] = p.Z();
                o[3] = d.X(); o[4] = d.Y(); o[5] = d.Z();
            }
            return n;
        },
        -1);
}

// Points put on a face's surface (the nearest point): for each, out gets
// u, v, the point and the outward normal (8 doubles).
int focc_face_project(const FShape* s, int face, int n, const double* xyz, double* out) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape faces;
            face_map(s->s, faces);
            if (face < 0 || face >= faces.Extent()) {
                set_err("no such face");
                return -1;
            }
            TopoDS_Face f = TopoDS::Face(faces(face + 1));
            opencascade::handle<ShapeAnalysis_Surface> an = new ShapeAnalysis_Surface(BRep_Tool::Surface(f));
            BRepAdaptor_Surface sa(f, Standard_True);
            bool rev = f.Orientation() == TopAbs_REVERSED;
            for (int k = 0; k < n; k++) {
                gp_Pnt q(xyz[3 * k], xyz[3 * k + 1], xyz[3 * k + 2]);
                gp_Pnt2d uv = an->ValueOfUV(q, 1e-7);
                double u = uv.X(), v = uv.Y();
                gp_Pnt p = sa.Value(u, v);
                BRepLProp_SLProps props(sa, u, v, 1, 1e-7);
                gp_Dir d(0, 0, 1);
                if (props.IsNormalDefined()) d = props.Normal();
                if (rev) d.Reverse();
                double* o = out + 8 * k;
                o[0] = u; o[1] = v;
                o[2] = p.X(); o[3] = p.Y(); o[4] = p.Z();
                o[5] = d.X(); o[6] = d.Y(); o[7] = d.Z();
            }
            return n;
        },
        -1);
}

int focc_buf_len(const FBuf* b) { return (int)b->b.size(); }
const uint8_t* focc_buf_data(const FBuf* b) { return b->b.data(); }
void focc_buf_free(FBuf* b) { delete b; }

}  // extern "C"
