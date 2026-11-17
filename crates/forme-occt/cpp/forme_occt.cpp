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
