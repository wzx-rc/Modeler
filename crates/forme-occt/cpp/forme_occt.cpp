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
#include <Geom2d_Curve.hxx>
#include <BRepLProp_SLProps.hxx>
#include <BRepTools.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepAlgoAPI_Splitter.hxx>
#include <BRepBndLib.hxx>
#include <BRepLib.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepOffsetAPI_MakeOffset.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <Bnd_Box.hxx>
#include <gp_Pln.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeSolid.hxx>
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
#include <GeomLProp_SLProps.hxx>
#include <GeomAPI_Interpolate.hxx>
#include <GeomAPI_ProjectPointOnSurf.hxx>
#include <GeomPlate_BuildPlateSurface.hxx>
#include <GeomPlate_CurveConstraint.hxx>
#include <GeomPlate_MakeApprox.hxx>
#include <Geom_BSplineSurface.hxx>
#include <GCPnts_AbscissaPoint.hxx>
#include <TColgp_HArray1OfPnt.hxx>
#include <TColStd_HArray1OfReal.hxx>
#include <BRepTools_Modification.hxx>
#include <BRepTools_Modifier.hxx>
#include <gp_Trsf2d.hxx>
#include <gp_Ax2d.hxx>
#include <TopTools_DataMapOfShapeInteger.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Tool.hxx>
#include <BRep_Builder.hxx>
#include <BRepFill_Filling.hxx>
#include <GeomAPI_PointsToBSplineSurface.hxx>
#include <GCE2d_MakeSegment.hxx>
#include <TColStd_Array2OfReal.hxx>
#include <ShapeFix_Face.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <Geom_BezierSurface.hxx>
#include <GC_MakeArcOfCircle.hxx>
#include <BRepTools_ReShape.hxx>
#include <ShapeFix_Shape.hxx>
#include <ShapeBuild_ReShape.hxx>
#include <Geom_SurfaceOfLinearExtrusion.hxx>
#include <Geom_SurfaceOfRevolution.hxx>
#include <ShapeFix_Edge.hxx>
#include <BRepAlgoAPI_Defeaturing.hxx>
#include <GeomAPI_IntCS.hxx>
#include <GeomAPI_ProjectPointOnCurve.hxx>
#include <functional>
#include <memory>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Line.hxx>
#include <Geom2d_Line.hxx>
#include <Geom2d_BSplineCurve.hxx>
#include <Geom2dAPI_Interpolate.hxx>
#include <TColgp_HArray1OfPnt2d.hxx>
#include <TColgp_Array1OfPnt2d.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <Precision.hxx>
#include <TColgp_Array2OfPnt.hxx>
#include <TColStd_Array2OfReal.hxx>
#include <TopTools_DataMapOfShapeShape.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <ShapeAnalysis_Surface.hxx>
#include <ShapeUpgrade_UnifySameDomain.hxx>
#include <BRepTools_History.hxx>
#include <TopTools_MapOfShape.hxx>
#include <BRepClass_FaceClassifier.hxx>
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
#include <gp_GTrsf.hxx>
#include <BRepBuilderAPI_GTransform.hxx>
#include <ShapeFix_Solid.hxx>
#include <ShapeAnalysis_FreeBounds.hxx>
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

// A copy of a shape with its tags (same face and edge order), for the
// algorithms that may change their input's tolerances in place (a fillet
// that fails can leave them so): the shapes the kernel keeps between builds
// stay as they were, else a later fillet on them may crash.
static FShape pristine(const FShape* s) {
    FShape c;
    c.s = BRepBuilderAPI_Copy(s->s).Shape();
    c.tags = s->tags;
    c.section = s->section;
    return c;
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

// FORME_OCCT_DEBUG=1: what BRepCheck finds wrong in a shape.
static void dump_check(const TopoDS_Shape& solid) {
    if (!getenv("FORME_OCCT_DEBUG")) return;
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

// Every cylinder given one frame for its axis line and radius: direct, the
// axis pointing the way of its largest positive component, the origin the
// axis point nearest the world's, X from the world axis least along it.
// Two faces on one cylinder then share their parameters, which
// ShapeUpgrade_UnifySameDomain needs to make them one face. Each face's
// pcurves follow: (u, v) -> (alpha +- u, c +- v); a mirror there turns the
// face and its wires over. The faces stay in the same order.
class CanonCylinders : public BRepTools_Modification {
public:
    struct Map { opencascade::handle<Geom_Surface> s; gp_Trsf2d t; bool rev; };
    TopTools_DataMapOfShapeInteger idx;
    std::vector<Map> maps;
    bool find(const TopoDS_Face& F, Map& out) {
        TopoDS_Face key = F;
        key.Orientation(TopAbs_FORWARD);
        if (idx.IsBound(key)) {
            int i = idx.Find(key);
            if (i < 0) return false;
            out = maps[i];
            return true;
        }
        TopLoc_Location L;
        opencascade::handle<Geom_Surface> S = BRep_Tool::Surface(F, L);
        opencascade::handle<Geom_CylindricalSurface> c = opencascade::handle<Geom_CylindricalSurface>::DownCast(S);
        if (c.IsNull()) { idx.Bind(key, -1); return false; }
        gp_Ax3 a = c->Position();
        if (!L.IsIdentity()) a.Transform(L.Transformation());
        gp_XYZ z = a.Direction().XYZ();
        double zs[3] = {z.X(), z.Y(), z.Z()};
        int big = 0, small = 0;
        for (int k = 1; k < 3; k++) {
            if (std::fabs(zs[k]) > std::fabs(zs[big])) big = k;
            if (std::fabs(zs[k]) < std::fabs(zs[small])) small = k;
        }
        gp_Dir Zc = zs[big] > 0 ? a.Direction() : a.Direction().Reversed();
        gp_XYZ O = a.Location().XYZ();
        O = O - Zc.XYZ() * O.Dot(Zc.XYZ());
        gp_XYZ w(small == 0, small == 1, small == 2);
        gp_Dir Xc(w - Zc.XYZ() * w.Dot(Zc.XYZ()));
        gp_Ax3 ac(gp_Pnt(O), Zc, Xc);
        double su = a.YDirection().XYZ().Dot(Zc.XYZ().Crossed(a.XDirection().XYZ())) > 0 ? 1 : -1;
        double sv = a.Direction().XYZ().Dot(Zc.XYZ()) > 0 ? 1 : -1;
        double alpha = std::atan2(a.XDirection().XYZ().Dot(ac.YDirection().XYZ()), a.XDirection().XYZ().Dot(ac.XDirection().XYZ()));
        double cc = (a.Location().XYZ() - O).Dot(Zc.XYZ());
        if (su > 0 && sv > 0 && std::fabs(alpha) < 1e-12 && std::fabs(cc) < 1e-12 && L.IsIdentity()) { idx.Bind(key, -1); return false; }
        gp_Trsf2d t;
        if (su < 0 && sv < 0) t.SetRotation(gp_Pnt2d(0, 0), M_PI);
        else if (su < 0) t.SetMirror(gp_Ax2d(gp_Pnt2d(0, 0), gp_Dir2d(0, 1)));
        else if (sv < 0) t.SetMirror(gp_Ax2d(gp_Pnt2d(0, 0), gp_Dir2d(1, 0)));
        gp_Trsf2d tr;
        tr.SetTranslation(gp_Vec2d(alpha, cc));
        Map m;
        m.s = new Geom_CylindricalSurface(ac, c->Radius());
        m.t = tr * t;
        m.rev = su * sv < 0;
        maps.push_back(m);
        idx.Bind(key, (int)maps.size() - 1);
        out = m;
        return true;
    }
    Standard_Boolean NewSurface(const TopoDS_Face& F, opencascade::handle<Geom_Surface>& S, TopLoc_Location& L, Standard_Real& Tol, Standard_Boolean& RevWires, Standard_Boolean& RevFace) override {
        Map m;
        if (!find(F, m)) return Standard_False;
        S = m.s;
        L.Identity();
        Tol = BRep_Tool::Tolerance(F);
        RevWires = m.rev;
        RevFace = m.rev;
        return Standard_True;
    }
    Standard_Boolean NewCurve(const TopoDS_Edge&, opencascade::handle<Geom_Curve>&, TopLoc_Location&, Standard_Real&) override { return Standard_False; }
    Standard_Boolean NewPoint(const TopoDS_Vertex&, gp_Pnt&, Standard_Real&) override { return Standard_False; }
    Standard_Boolean NewCurve2d(const TopoDS_Edge& E, const TopoDS_Face& F, const TopoDS_Edge&, const TopoDS_Face&, opencascade::handle<Geom2d_Curve>& C, Standard_Real& Tol) override {
        Map m;
        if (!find(F, m)) return Standard_False;
        double f, l;
        opencascade::handle<Geom2d_Curve> c = BRep_Tool::CurveOnSurface(E, F, f, l);
        if (c.IsNull()) return Standard_False;
        C = opencascade::handle<Geom2d_Curve>::DownCast(c->Transformed(m.t));
        Tol = BRep_Tool::Tolerance(E);
        return Standard_True;
    }
    Standard_Boolean NewParameter(const TopoDS_Vertex&, const TopoDS_Edge&, Standard_Real&, Standard_Real&) override { return Standard_False; }
    GeomAbs_Shape Continuity(const TopoDS_Edge& E, const TopoDS_Face& F1, const TopoDS_Face& F2, const TopoDS_Edge&, const TopoDS_Face&, const TopoDS_Face&) override { return BRep_Tool::Continuity(E, F1, F2); }
};
static TopoDS_Shape canon_cylinders(const TopoDS_Shape& s) {
    opencascade::handle<CanonCylinders> m = new CanonCylinders();
    BRepTools_Modifier md(s, m);
    if (!md.IsDone()) return s;
    return md.ModifiedShape(s);
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
// start, middle, end), 2 a chamfer (a straight side from start to end), 3 a
// conic (a fillet with tension: start, a point of it, end; the corner is its
// middle pole); on a smooth point, 4 its curve the conic of rho abm[6 i + 2]
// (an ellipse's quarter) instead of the quadratic; 5 the side from point i to
// the next a cubic Bezier, its inner controls abm[6 i..6 i + 4] (a spline).
// The outline of focc_prism_cut as a wire at height z0 (XY plane); `open`:
// from the first point to the last, its ends plain points (a path's). A
// null wire (and the error set) when it cannot be made.
static TopoDS_Wire outline_wire(const double* xy, const int* smooth, const int* cut, const double* abm, int n, double z0, bool open) {
    std::vector<gp_Pnt> p(n);
    for (int i = 0; i < n; i++) p[i] = gp_Pnt(xy[2 * i], xy[2 * i + 1], z0);
    auto mid = [](const gp_Pnt& a, const gp_Pnt& b) { return gp_Pnt((a.X() + b.X()) / 2, (a.Y() + b.Y()) / 2, (a.Z() + b.Z()) / 2); };
    // where each point's piece starts and ends (the point itself for a corner)
    std::vector<TopoDS_Vertex> vin(n), vout(n);
    for (int i = 0; i < n; i++) {
        const gp_Pnt& prev = p[(i + n - 1) % n];
        const gp_Pnt& next = p[(i + 1) % n];
        bool end = open && (i == 0 || i == n - 1);
        bool round = !end && smooth[i] && prev.Distance(p[i]) > 1e-6 && next.Distance(p[i]) > 1e-6;
        int ck = cut && !end && !round && cut[i] != 4 && cut[i] != 5 ? cut[i] : 0;
        if (ck) {
            vin[i] = BRepBuilderAPI_MakeVertex(gp_Pnt(abm[6 * i], abm[6 * i + 1], z0));
            vout[i] = BRepBuilderAPI_MakeVertex(gp_Pnt(abm[6 * i + 4], abm[6 * i + 5], z0));
        } else if (round) {
            vin[i] = BRepBuilderAPI_MakeVertex(mid(prev, p[i]));
            vout[i] = BRepBuilderAPI_MakeVertex(mid(p[i], next));
        } else {
            vin[i] = vout[i] = BRepBuilderAPI_MakeVertex(p[i]);
        }
    }
    // two smooth points in a row meet at the middle of their edge: one vertex
    for (int i = 0; i < (open ? n - 1 : n); i++) {
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
        if (open && i == n - 1) break;
        if (!vin[i].IsSame(vout[i]) && ck == 2) {
            mw.Add(BRepBuilderAPI_MakeEdge(vin[i], vout[i]).Edge());
        } else if (!vin[i].IsSame(vout[i]) && ck == 3) {
            // a conic (a fillet with tension): the rational quadratic
            // through start and end, the corner its middle pole, the
            // weight from where its middle lies (rho / (1 - rho))
            gp_Pnt a = BRep_Tool::Pnt(vin[i]), b = BRep_Tool::Pnt(vout[i]);
            gp_Pnt m(abm[6 * i + 2], abm[6 * i + 3], z0);
            gp_Pnt mid((a.X() + b.X()) / 2, (a.Y() + b.Y()) / 2, z0);
            double full = mid.Distance(p[i]);
            double rho = full > 1e-12 ? mid.Distance(m) / full : 0.5;
            if (rho < 0.02) rho = 0.02;
            if (rho > 0.98) rho = 0.98;
            TColgp_Array1OfPnt poles(1, 3);
            poles(1) = a;
            poles(2) = p[i];
            poles(3) = b;
            TColStd_Array1OfReal weights(1, 3);
            weights(1) = 1.0;
            weights(2) = rho / (1.0 - rho);
            weights(3) = 1.0;
            opencascade::handle<Geom_BezierCurve> c = new Geom_BezierCurve(poles, weights);
            mw.Add(BRepBuilderAPI_MakeEdge(c, vin[i], vout[i]).Edge());
        } else if (!vin[i].IsSame(vout[i]) && ck == 1) {
            GC_MakeArcOfCircle arc(BRep_Tool::Pnt(vin[i]), gp_Pnt(abm[6 * i + 2], abm[6 * i + 3], z0), BRep_Tool::Pnt(vout[i]));
            if (!arc.IsDone()) {
                set_err("a rounded corner could not be made");
                return TopoDS_Wire();
            }
            mw.Add(BRepBuilderAPI_MakeEdge(arc.Value(), vin[i], vout[i]).Edge());
        } else if (!vin[i].IsSame(vout[i])) {
            TColgp_Array1OfPnt poles(1, 3);
            poles(1) = BRep_Tool::Pnt(vin[i]);
            poles(2) = p[i];
            poles(3) = BRep_Tool::Pnt(vout[i]);
            opencascade::handle<Geom_BezierCurve> c;
            double rho = ck == 4 ? abm[6 * i + 2] : 0.0;
            if (rho > 1e-6) {
                if (rho < 0.02) rho = 0.02;
                if (rho > 0.98) rho = 0.98;
                TColStd_Array1OfReal weights(1, 3);
                weights(1) = 1.0;
                weights(2) = rho / (1.0 - rho);
                weights(3) = 1.0;
                c = new Geom_BezierCurve(poles, weights);
            } else {
                c = new Geom_BezierCurve(poles);
            }
            mw.Add(BRepBuilderAPI_MakeEdge(c, vin[i], vout[i]).Edge());
        }
        int j = (i + 1) % n;
        if (ck == 5 && !vout[i].IsSame(vin[j])) {
            // a spline's span (Profile::spline): the cubic Bezier from this
            // point to the next, its inner controls abm[6 i..6 i + 4]
            TColgp_Array1OfPnt poles(1, 4);
            poles(1) = BRep_Tool::Pnt(vout[i]);
            poles(2) = gp_Pnt(abm[6 * i], abm[6 * i + 1], z0);
            poles(3) = gp_Pnt(abm[6 * i + 2], abm[6 * i + 3], z0);
            poles(4) = BRep_Tool::Pnt(vin[j]);
            opencascade::handle<Geom_BezierCurve> c = new Geom_BezierCurve(poles);
            mw.Add(BRepBuilderAPI_MakeEdge(c, vout[i], vin[j]).Edge());
        } else if (!vout[i].IsSame(vin[j])) mw.Add(BRepBuilderAPI_MakeEdge(vout[i], vin[j]).Edge());
    }
    if (!mw.IsDone()) {
        set_err(open ? "the path could not be made" : "the outline is not a closed line");
        return TopoDS_Wire();
    }
    return mw.Wire();
}

// The outline of focc_prism_cut as a flat face at height z0 (XY plane);
// a null face (and the error set) when it cannot be made.
static TopoDS_Face outline_face(const double* xy, const int* smooth, const int* cut, const double* abm, int n, double z0) {
    TopoDS_Wire w = outline_wire(xy, smooth, cut, abm, n, z0, false);
    if (w.IsNull()) return TopoDS_Face();
    BRepBuilderAPI_MakeFace mf(w, Standard_True);
    if (!mf.IsDone()) {
        set_err("the outline is not flat and closed");
        return TopoDS_Face();
    }
    return mf.Face();
}

FShape* focc_prism_cut(const double* xy, const int* smooth, const int* cut, const double* abm, int n, double h) {
    return guard(
        [&]() -> FShape* {
            if (n < 3 || h <= 0) {
                set_err("a profile needs 3 points and a thickness");
                return nullptr;
            }
            TopoDS_Face face = outline_face(xy, smooth, cut, abm, n, -h);
            if (face.IsNull()) return nullptr;
            BRepPrimAPI_MakePrism prism(face, gp_Vec(0, 0, 2 * h));
            TopoDS_Shape r = prism.Shape();
            if (!valid(r)) {
                set_err("the outline crosses itself");
                return nullptr;
            }
            return wrap(r);
        },
        (FShape*)nullptr);
}

// The same outline as a flat face in the XY plane (a closed curve's region,
// the Curves set's 2D booleans).
FShape* focc_region_cut(const double* xy, const int* smooth, const int* cut, const double* abm, int n) {
    return guard(
        [&]() -> FShape* {
            if (n < 3) {
                set_err("a region needs 3 points");
                return nullptr;
            }
            TopoDS_Face face = outline_face(xy, smooth, cut, abm, n, 0.0);
            if (face.IsNull()) return nullptr;
            if (!valid(face)) {
                set_err("the outline crosses itself");
                return nullptr;
            }
            return wrap(face);
        },
        (FShape*)nullptr);
}

// A sculpted form (Forms): the bicubic BSpline surface through `poles`
// (world, nu × nv, row major: poles[(j * nu + i) * 3]), periodic round
// (u: nu columns, uniform knots), clamped from bottom to top (v: nv rows,
// uniform inside); its first and last rows each one point, so it closes on
// itself: made a solid, facing out.
FShape* focc_form(const double* poles, int nu, int nv) {
    return guard(
        [&]() -> FShape* {
            if (nu < 4 || nv < 4) {
                set_err("a form needs 4 × 4 poles at least");
                return nullptr;
            }
            TColgp_Array2OfPnt P(1, nu, 1, nv);
            for (int j = 0; j < nv; j++)
                for (int i = 0; i < nu; i++) {
                    const double* q = poles + 3 * (j * nu + i);
                    P(i + 1, j + 1) = gp_Pnt(q[0], q[1], q[2]);
                }
            // u: periodic, nu + 1 knots of multiplicity 1
            TColStd_Array1OfReal uk(1, nu + 1);
            TColStd_Array1OfInteger um(1, nu + 1);
            for (int i = 0; i <= nu; i++) {
                uk(i + 1) = i;
                um(i + 1) = 1;
            }
            // v: clamped cubic, nv - 2 knots, the ends of multiplicity 4
            int nk = nv - 2;
            TColStd_Array1OfReal vk(1, nk);
            TColStd_Array1OfInteger vm(1, nk);
            for (int k = 0; k < nk; k++) {
                vk(k + 1) = k;
                vm(k + 1) = (k == 0 || k == nk - 1) ? 4 : 1;
            }
            opencascade::handle<Geom_BSplineSurface> srf = new Geom_BSplineSurface(P, uk, vk, um, vm, 3, 3, Standard_True, Standard_False);
            BRepBuilderAPI_MakeFace mf(srf, 1e-6);
            if (!mf.IsDone()) {
                set_err("the form's surface could not be made");
                return nullptr;
            }
            BRep_Builder b;
            TopoDS_Shell sh;
            b.MakeShell(sh);
            b.Add(sh, mf.Face());
            sh.Closed(Standard_True);
            TopoDS_Solid so;
            b.MakeSolid(so);
            b.Add(so, sh);
            ShapeFix_Solid fix(so);
            fix.Perform();
            TopoDS_Shape r = fix.Shape();
            TopoDS_Shape one;
            for (TopExp_Explorer x(r, TopAbs_SOLID); x.More(); x.Next()) one = x.Current();
            if (one.IsNull()) {
                set_err("the form does not close");
                return nullptr;
            }
            // facing out
            TopoDS_Solid sol = TopoDS::Solid(one);
            BRepLib::OrientClosedSolid(sol);
            one = sol;
            if (!valid(one)) {
                dump_check(one);
                set_err("the form folds through itself");
                return nullptr;
            }
            return wrap(one);
        },
        (FShape*)nullptr);
}

// The same outline as a wire in the XY plane, open (a path: from its first
// point to its last) or closed.
FShape* focc_outline_wire(const double* xy, const int* smooth, const int* cut, const double* abm, int n, int open) {
    return guard(
        [&]() -> FShape* {
            if (n < 2) {
                set_err("a curve needs 2 points");
                return nullptr;
            }
            TopoDS_Wire w = outline_wire(xy, smooth, cut, abm, n, 0.0, open != 0);
            if (w.IsNull()) return nullptr;
            return wrap(w);
        },
        (FShape*)nullptr);
}

// `s` (a face: a solid; a wire: a sheet) turned by `angle` (radians, 2 pi a
// full turn) round the axis through o along d (Revolve, 3ds Max's Lathe).
FShape* focc_revolve(const FShape* s, const double* o, const double* d, double angle) {
    return guard(
        [&]() -> FShape* {
            gp_Ax1 ax(gp_Pnt(o[0], o[1], o[2]), gp_Dir(d[0], d[1], d[2]));
            bool full = angle >= 2 * M_PI - 1e-9;
            BRepPrimAPI_MakeRevol mk(s->s, ax, full ? 2 * M_PI : angle, Standard_True);
            mk.Build();
            if (!mk.IsDone()) {
                set_err("the curve could not be turned round its axis");
                return nullptr;
            }
            TopoDS_Shape r = mk.Shape();
            // one solid when the face's turn made one (its own compound)
            int solids = 0;
            TopoDS_Shape one;
            for (TopExp_Explorer x(r, TopAbs_SOLID); x.More(); x.Next()) {
                solids++;
                one = x.Current();
            }
            if (solids == 1) r = one;
            if (!valid(r)) {
                ShapeFix_Shape fix(r);
                fix.Perform();
                r = fix.Shape();
                if (!valid(r)) {
                    set_err("the turned curve crosses its axis or itself");
                    return nullptr;
                }
            }
            return wrap(r);
        },
        (FShape*)nullptr);
}

// A loft through sections (wires, or faces: their outer wire), in order:
// a solid when `solid` (all closed), else a sheet; `ruled`: straight
// between sections (Loft).
FShape* focc_loft(const FShape* const* secs, int n, int solid, int ruled) {
    return guard(
        [&]() -> FShape* {
            if (n < 2) {
                set_err("a loft needs 2 curves at least");
                return nullptr;
            }
            BRepOffsetAPI_ThruSections ts(solid != 0, ruled != 0, 1e-6);
            ts.CheckCompatibility(Standard_True);
            for (int i = 0; i < n; i++) {
                TopoDS_Wire w;
                TopExp_Explorer fx(secs[i]->s, TopAbs_FACE);
                if (fx.More()) {
                    w = BRepTools::OuterWire(TopoDS::Face(fx.Current()));
                } else {
                    TopExp_Explorer wx(secs[i]->s, TopAbs_WIRE);
                    if (wx.More()) w = TopoDS::Wire(wx.Current());
                }
                if (w.IsNull()) {
                    set_err("a curve of the loft has no line");
                    return nullptr;
                }
                ts.AddWire(w);
            }
            ts.Build();
            if (!ts.IsDone()) {
                set_err("the loft could not be made through these curves");
                return nullptr;
            }
            TopoDS_Shape r = ts.Shape();
            if (!valid(r)) {
                ShapeFix_Shape fix(r);
                fix.Perform();
                r = fix.Shape();
                if (!valid(r)) {
                    set_err("the loft twists through itself");
                    return nullptr;
                }
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

// BRepAlgoAPI_BuilderAlgo::SimplifyResult with the edges between two
// faces of one tag in an input kept (the Knife's lines: the pieces of a
// face it cut keep its tag).
struct OpAccess : BRepAlgoAPI_BooleanOperation {
    static TopoDS_Shape& shape(BRepAlgoAPI_BooleanOperation& o) { return o.*(&OpAccess::myShape); }
    static opencascade::handle<BRepTools_History>& history(BRepAlgoAPI_BooleanOperation& o) { return o.*(&OpAccess::myHistory); }
};

static void simplify_keeping(BRepAlgoAPI_BooleanOperation& op, const FShape* a, const FShape* b) {
    ShapeUpgrade_UnifySameDomain u(op.Shape(), Standard_True, Standard_True, Standard_True);
    u.AllowInternalEdges(Standard_False);
    TopTools_IndexedMapOfShape rm;
    TopExp::MapShapes(op.Shape(), TopAbs_EDGE, rm);
    for (const FShape* in : {a, b}) {
        if (!in || in->tags.empty()) continue;
        TopTools_IndexedMapOfShape fm;
        face_map(in->s, fm);
        TopTools_IndexedDataMapOfShapeListOfShape e2f;
        TopExp::MapShapesAndAncestors(in->s, TopAbs_EDGE, TopAbs_FACE, e2f);
        for (int i = 1; i <= e2f.Extent(); i++) {
            const TopTools_ListOfShape& fl = e2f(i);
            if (fl.Extent() != 2 || fl.First().IsSame(fl.Last())) continue;
            int fa = fm.FindIndex(fl.First()), fb = fm.FindIndex(fl.Last());
            if (fa < 1 || fb < 1 || fa > (int)in->tags.size() || fb > (int)in->tags.size()) continue;
            long long ta = in->tags[fa - 1], tb = in->tags[fb - 1];
            if (ta < 0 || ta != tb) continue;
            const TopoDS_Shape& e = e2f.FindKey(i);
            if (rm.Contains(e)) u.KeepShape(e);
            for (TopTools_ListOfShape::Iterator it(op.Modified(e)); it.More(); it.Next())
                if (rm.Contains(it.Value())) u.KeepShape(it.Value());
        }
    }
    u.Build();
    OpAccess::shape(op) = u.Shape();
    if (op.HasHistory()) OpAccess::history(op)->Merge(u.History());
}

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
    // become one face, without a seam), but never two pieces of one face
    // in a or b (a Knife's line: 20 nov., an Extrude or a Cube added later
    // made them one again)
    simplify_keeping(op, a, b);
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

FShape* focc_common(const FShape* a, const FShape* b) {
    return guard(
        [&] {
            BRepAlgoAPI_Common op;
            TopTools_ListOfShape args, tools;
            args.Append(a->s);
            tools.Append(b->s);
            op.SetArguments(args);
            op.SetTools(tools);
            return boolean(op, "intersection", a, b);
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
            const FShape copy = pristine(s);
            const FShape* s = &copy;
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

// A fillet with tension (Romain 19 nov.: "SHIFT maintenu : tension set"):
// each face tagged `tag` that is a round fillet face (a cylinder along a
// straight edge, a torus along a circular one) has its circular sections
// replaced by conics (rational quadratics: the section's two ends, the
// corner of its two tangents as middle pole, weight rho / (1 - rho)): a
// cylinder becomes the conic swept along its axis, a torus the conic turned
// round its axis. The edges along the fillet (where it touches its two
// faces) are kept, so the faces round it do not change. Its ends: a flat
// face (a cylinder's at any angle: the swept conic cut by that plane is
// still a conic; a torus's through its axis), or another face of the same
// fillet made conic too (a run of tangent edges: the shared section is the
// same conic on both sides). A face whose end is anything else (a blended
// corner, a curved face) stays round, and so do the faces of its run next
// to it. `done` gets how many faces were made conic, `total` how many
// faces carry the tag.
namespace {
struct TenFace {
    TopoDS_Face f;
    long long tag;
    int which;     // its fillet among those asked
    double rho;
    bool torus;
    gp_Ax3 frame;  // the cylinder's or torus's own frame (Z its axis)
    double u0, u1, v0, v1;
    opencascade::handle<Geom_BezierCurve> gen;  // the conic at the start of the sweep
    std::vector<TopoDS_Edge> contacts, sections;
    // any other surface (a fillet run along a curve that is not a line or
    // a circle): its new surface made here, the conic running in u (else v)
    bool general = false;
    bool sec_u = true;
    opencascade::handle<Geom_Surface> gsurf;
    // a general face's sections kept as they are (a circle shared with a
    // round face left so): the conic there is that circle, its tension
    // going over to its own along the face
    std::vector<bool> kept;
    // a general face of a fillet not asked, next to one made conic (a
    // corner OpenCascade made between them): remade to follow it, round
    // elsewhere (rho < 0)
    bool follower = false;
    // its contact on a cylinder made conic (index in contacts, else -1),
    // and that contact's new edge
    int follow = -1;
    TopoDS_Edge follow_edge;
    opencascade::handle<Geom2d_Curve> follow_pc;  // on the cylinder's new surface
    double follow_u = 0;                          // its row on this face's (u = 0 or 1)
    // a general face's end shared with a cylinder made conic: that
    // cylinder's new section there, taken as this face's end row exactly
    opencascade::handle<Geom_BezierCurve> endc[2];
};

// How a general face's contact follows a cylinder made conic: the old
// cylinder's frame and the new surface (the conic `gen`, at axial place v0,
// swept along the axis); `side` is the row it is (0: u0 / v0, 1: u1 / v1).
struct TenFollow {
    int side = 0;
    TopoDS_Face fixed;  // the face across its other contact
    gp_Ax3 frame;
    double v0 = 0;
    opencascade::handle<Geom_BezierCurve> gen;
    std::vector<gp_Pnt2d> uv;  // the contact's samples on the new surface (t, along)
};

// The rational quadratic from a (tangent ta, pointing along the section)
// to b (tangent tb, pointing along the section, on to b): its middle pole
// where the two tangent lines meet.
opencascade::handle<Geom_BezierCurve> ten_conic(const gp_Pnt& a, const gp_Vec& ta, const gp_Pnt& b, const gp_Vec& tb, double w) {
    // closest points of a + s ta and b + t tb
    gp_Vec d(b, a);
    double A = ta.Dot(ta), B = ta.Dot(tb), C = tb.Dot(tb), D = ta.Dot(d), E = tb.Dot(d);
    double den = A * C - B * B;
    if (std::fabs(den) < 1e-12 * A * C) return nullptr;
    double s = (B * E - C * D) / den;
    gp_Pnt c = a.Translated(ta * s);
    TColgp_Array1OfPnt poles(1, 3);
    poles(1) = a;
    poles(2) = c;
    poles(3) = b;
    TColStd_Array1OfReal wt(1, 3);
    wt(1) = 1.0;
    wt(2) = w;
    wt(3) = 1.0;
    return new Geom_BezierCurve(poles, wt);
}

// The same conic moved by an affine map (a transform, or a projection
// along `dir` onto a plane), its weights kept.
opencascade::handle<Geom_BezierCurve> ten_map(const opencascade::handle<Geom_BezierCurve>& g, const std::function<gp_Pnt(const gp_Pnt&)>& m, bool reversed) {
    TColgp_Array1OfPnt poles(1, 3);
    TColStd_Array1OfReal wt(1, 3);
    for (int i = 1; i <= 3; i++) {
        int k = reversed ? 4 - i : i;
        poles(i) = m(g->Pole(k));
        wt(i) = g->Weight(k);
    }
    return new Geom_BezierCurve(poles, wt);
}

bool ten_near_mod(double a, double b) {
    double d = std::remainder(a - b, 2 * M_PI);
    return std::fabs(d) < 1e-6;
}
}  // namespace

// Whether an edge is a circular arc (its curve a circle, or any curve
// that runs on one: a fillet's end made as a spline), by samples.
static bool edge_is_arc(const TopoDS_Edge& e) {
    BRepAdaptor_Curve c(e);
    if (c.GetType() == GeomAbs_Circle) return true;
    double f0 = c.FirstParameter(), f1 = c.LastParameter();
    gp_Pnt a = c.Value(f0), b = c.Value(0.5 * (f0 + f1)), d = c.Value(f1);
    gp_Vec ab(a, b), ad(a, d);
    gp_Vec n = ab.Crossed(ad);
    double n2 = n.SquareMagnitude();
    if (n2 < 1e-16) return false;
    // the circle through a, b, d
    gp_Vec to = (n.Crossed(ab) * ad.SquareMagnitude() + ad.Crossed(n) * ab.SquareMagnitude()) / (2.0 * n2);
    gp_Pnt o = a.Translated(to);
    double r = to.Magnitude();
    gp_Dir nd(n);
    double tol = std::max(1e-6, 1e-6 * r);
    for (int k = 1; k < 32; k++) {
        gp_Pnt p = c.Value(f0 + (f1 - f0) * k / 32.0);
        if (std::fabs(p.Distance(o) - r) > tol || std::fabs(gp_Vec(o, p).Dot(gp_Vec(nd))) > tol) return false;
    }
    return true;
}

// The tightest radius of curvature along the rational quadratic a, k, b
// (weights 1, w, 1), by samples.
static double conic_min_radius(const gp_Pnt& a, const gp_Pnt& k, const gp_Pnt& b, double w) {
    double best = 1e100;
    const int M = 200;
    for (int i = 0; i <= M; i++) {
        double t = (double)i / M, u = 1.0 - t;
        gp_XYZ N = a.XYZ() * (u * u) + k.XYZ() * (w * 2 * t * u) + b.XYZ() * (t * t);
        gp_XYZ N1 = a.XYZ() * (-2 * u) + k.XYZ() * (w * (2 - 4 * t)) + b.XYZ() * (2 * t);
        gp_XYZ N2 = a.XYZ() * 2.0 + k.XYZ() * (-4 * w) + b.XYZ() * 2.0;
        double D = u * u + w * 2 * t * u + t * t, D1 = -2 * u + w * (2 - 4 * t) + 2 * t, D2 = 4 - 4 * w;
        gp_XYZ P = N / D;
        gp_XYZ P1 = (N1 - P * D1) / D;
        gp_XYZ P2 = (N2 - P1 * (2 * D1) - P * D2) / D;
        double sp = P1.Modulus(), c = P1.Crossed(P2).Modulus();
        if (c > 1e-15) best = std::min(best, sp * sp * sp / c);
    }
    return best;
}

// The surface through three rows of points sampled at the same places
// along a fillet (one contact, the conics' middle poles, the other contact):
// each row a cubic through its samples, the conic of weight w across
// between them (rational, degree 2 in u with weights 1, w, 1). `cr` gets the
// three row curves (cr[0] and cr[2] are its edges at u = 0 and 1).
// `ws`, when given, is the weight at each sample (a tension changing along
// the face): the middle row is then made in homogeneous space (w K and w
// each through their samples), so each section is the conic of its weight
// at the samples.
static opencascade::handle<Geom_BSplineSurface> ten_rows(opencascade::handle<TColgp_HArray1OfPnt> rows[3], const opencascade::handle<TColStd_HArray1OfReal>& par, double w, opencascade::handle<Geom_BSplineCurve> cr[3], const std::vector<double>* ws = nullptr) {
    int n = rows[1]->Length();
    if (ws && (int)ws->size() != n) return nullptr;
    opencascade::handle<Geom_BSplineCurve> cw;  // the weights, in x
    if (ws) {
        opencascade::handle<TColgp_HArray1OfPnt> h = new TColgp_HArray1OfPnt(1, n), hw = new TColgp_HArray1OfPnt(1, n);
        for (int j = 1; j <= n; j++) {
            double wj = (*ws)[j - 1];
            h->SetValue(j, gp_Pnt(rows[1]->Value(j).XYZ() * wj));
            hw->SetValue(j, gp_Pnt(wj, 0, 0));
        }
        rows[1] = h;
        GeomAPI_Interpolate ip(hw, par, Standard_False, 1e-9);
        ip.Perform();
        if (!ip.IsDone()) return nullptr;
        cw = ip.Curve();
    }
    // a row that is one point (a contact shrunk to a point): its poles all
    // that point, on the knots of the others (the same parameters)
    bool point[3];
    for (int i = 0; i < 3; i++) {
        point[i] = true;
        for (int j = 2; j <= n && point[i]; j++) point[i] = rows[i]->Value(j).Distance(rows[i]->Value(1)) < 1e-12;
        if (point[i]) continue;
        GeomAPI_Interpolate ip(rows[i], par, Standard_False, 1e-9);
        ip.Perform();
        if (!ip.IsDone()) return nullptr;
        cr[i] = ip.Curve();
    }
    if (point[1] || (point[0] && point[2])) return nullptr;
    for (int i = 0; i < 3; i += 2) {
        if (!point[i]) continue;
        cr[i] = opencascade::handle<Geom_BSplineCurve>::DownCast(cr[1]->Copy());
        for (int j = 1; j <= cr[i]->NbPoles(); j++) cr[i]->SetPole(j, rows[i]->Value(1));
    }
    int m = cr[0]->NbPoles();
    if (cr[1]->NbPoles() != m || cr[2]->NbPoles() != m) return nullptr;
    if (!cw.IsNull() && cw->NbPoles() != m) return nullptr;
    TColgp_Array2OfPnt poles(1, 3, 1, m);
    TColStd_Array2OfReal wts(1, 3, 1, m);
    double wi[3] = {1.0, w, 1.0};
    for (int i = 0; i < 3; i++)
        for (int j = 1; j <= m; j++) {
            if (i == 1 && !cw.IsNull()) {
                double wj = cw->Pole(j).X();
                if (wj < 1e-3) return nullptr;
                poles.SetValue(2, j, gp_Pnt(cr[1]->Pole(j).XYZ() / wj));
                wts.SetValue(2, j, wj);
                continue;
            }
            poles.SetValue(i + 1, j, cr[i]->Pole(j));
            wts.SetValue(i + 1, j, wi[i]);
        }
    TColStd_Array1OfReal uk(1, 2);
    TColStd_Array1OfInteger um(1, 2);
    uk(1) = 0.0;
    uk(2) = 1.0;
    um(1) = um(2) = 3;
    return new Geom_BSplineSurface(poles, wts, uk, cr[0]->Knots(), um, cr[0]->Multiplicities(), 2, cr[0]->Degree());
}

// A fillet face on any other surface (run along a curve that is neither a
// line nor a circle: a conic fillet's edge, a spline): its two contacts are
// isolines in one parameter, its ends isolines in the other. Its new
// surface has the conic of weight w between the contacts (from their
// tangents across) at every place along it: rational, degree 2 across,
// cubic along through samples of the contacts. `own` tells an edge shared
// with another face of the same fillet: that edge is an end, not a contact
// (a short fillet run up a mitre between two others is wider than long).
// False when it is not shaped so.
static opencascade::handle<Geom_BSplineSurface> ten_general_surf(const TenFace& tf, double rho0, double rho1, TenFollow* fo);
static bool ten_general(TenFace& tf, double w, const std::function<bool(const TopoDS_Edge&)>& own) {
    tf.general = true;
    tf.torus = false;
    BRepTools::UVBounds(tf.f, tf.u0, tf.u1, tf.v0, tf.v1);
    BRepAdaptor_Surface sa(tf.f, Standard_False);
    double tu = 1e-6 * std::max(1.0, tf.u1 - tf.u0), tv = 1e-6 * std::max(1.0, tf.v1 - tf.v0);
    std::vector<TopoDS_Edge> uiso, viso;  // constant u, constant v
    // a contact may shrink to a point (a fillet rolled round an edge curved
    // tighter than itself: a corner OpenCascade closes on a point): that
    // degenerated edge's side holds the contacts
    int degen_u = 0, degen_v = 0;
    for (TopExp_Explorer ex(tf.f, TopAbs_EDGE); ex.More(); ex.Next()) {
        TopoDS_Edge e = TopoDS::Edge(ex.Current());
        if (BRep_Tool::IsClosed(e, tf.f)) return false;
        bool deg = BRep_Tool::Degenerated(e);
        bool seen = false;
        for (auto& x : uiso) seen |= x.IsSame(e);
        for (auto& x : viso) seen |= x.IsSame(e);
        if (seen) continue;
        double f0, f1;
        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
        if (pc.IsNull()) return false;
        gp_Pnt2d p0 = pc->Value(f0), p1 = pc->Value(f1), pm = pc->Value(0.5 * (f0 + f1));
        bool cu = std::fabs(p0.X() - p1.X()) < tu && std::fabs(p0.X() - pm.X()) < tu && (std::fabs(p0.X() - tf.u0) < tu || std::fabs(p0.X() - tf.u1) < tu);
        bool cv = std::fabs(p0.Y() - p1.Y()) < tv && std::fabs(p0.Y() - pm.Y()) < tv && (std::fabs(p0.Y() - tf.v0) < tv || std::fabs(p0.Y() - tf.v1) < tv);
        if (cu == cv) return false;
        (cu ? uiso : viso).push_back(e);
        if (deg) (cu ? degen_u : degen_v)++;
    }
    if (degen_u + degen_v > 1) return false;
    auto both_ends = [&](const std::vector<TopoDS_Edge>& es, bool u) {
        if (es.size() != 2) return false;
        double c[2];
        for (int k = 0; k < 2; k++) {
            double f0, f1;
            opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(es[k], tf.f, f0, f1);
            c[k] = u ? pc->Value(f0).X() : pc->Value(f0).Y();
        }
        return std::fabs(c[0] - c[1]) > (u ? tu : tv);
    };
    auto len = [](const std::vector<TopoDS_Edge>& es) {
        double l = 0;
        for (auto& e : es)
            if (!BRep_Tool::Degenerated(e)) l += GCPnts_AbscissaPoint::Length(BRepAdaptor_Curve(e));
        return l;
    };
    bool u_contacts = both_ends(uiso, true) && viso.size() <= 2;
    bool v_contacts = both_ends(viso, false) && uiso.size() <= 2;
    if (degen_u && !u_contacts) return false;
    if (degen_v && !v_contacts) return false;
    if (degen_u) v_contacts = false;
    if (degen_v) u_contacts = false;
    if (u_contacts && v_contacts) {
        // the ends are the pair of arcs, else the pair that meets the
        // fillet's own faces, else the contacts run along the fillet: the
        // longer pair
        auto owns = [&](const std::vector<TopoDS_Edge>& es) {
            for (auto& e : es)
                if (own(e)) return true;
            return false;
        };
        // (a fillet's sections are arcs: the pair of arcs first)
        auto arcs = [&](const std::vector<TopoDS_Edge>& es) {
            for (auto& e : es)
                if (!edge_is_arc(e)) return false;
            return true;
        };
        bool au = arcs(uiso), av = arcs(viso);
        bool ou = owns(uiso), ov = owns(viso);
        if (au != av) (au ? u_contacts : v_contacts) = false;
        else if (ou != ov) (ou ? u_contacts : v_contacts) = false;
        else if (len(uiso) >= len(viso)) v_contacts = false;
        else u_contacts = false;
    }
    if (!u_contacts && !v_contacts) return false;
    tf.sec_u = u_contacts;
    tf.contacts = u_contacts ? uiso : viso;
    tf.sections = u_contacts ? viso : uiso;
    tf.kept.assign(tf.sections.size(), false);
    double rho = w / (1.0 + w);
    tf.gsurf = ten_general_surf(tf, rho, rho, nullptr);
    return !tf.gsurf.IsNull();
}

// The place on a cylinder made conic (`fo`) where a point of the old one
// goes: the same place along the axis, the same direction out from it.
static gp_Pnt ten_follow_point(const TenFollow& fo, const gp_Pnt& p, gp_Vec* normal, gp_Pnt2d* uv) {
    gp_Vec z(fo.frame.Direction()), x(fo.frame.XDirection()), y(fo.frame.YDirection());
    gp_Vec op(fo.frame.Location(), p);
    double v = op.Dot(z), phi = std::atan2(op.Dot(y), op.Dot(x));
    gp_Pnt c0 = fo.frame.Location().Translated(z * fo.v0);
    auto ang = [&](double t) {
        gp_Vec q(c0, fo.gen->Value(t));
        return std::remainder(std::atan2(q.Dot(y), q.Dot(x)) - phi, 2 * M_PI);
    };
    double lo = 0.0, hi = 1.0, flo = ang(lo), fhi = ang(hi), t;
    if (std::fabs(flo) < 1e-12) t = 0.0;
    else if (std::fabs(fhi) < 1e-12) t = 1.0;
    else if (flo * fhi > 0) t = std::fabs(flo) < std::fabs(fhi) ? 0.0 : 1.0;  // (just past an end)
    else {
        for (int k = 0; k < 60; k++) {
            double m = 0.5 * (lo + hi), fm = ang(m);
            if ((fm < 0) == (flo < 0)) {
                lo = m;
                flo = fm;
            } else hi = m;
        }
        t = 0.5 * (lo + hi);
    }
    gp_Pnt g;
    gp_Vec dg;
    fo.gen->D1(t, g, dg);
    if (normal) *normal = dg.Crossed(z);
    if (uv) *uv = gp_Pnt2d(t, v - fo.v0);
    return g.Translated(z * (v - fo.v0));
}

// A general fillet face's new surface (see ten_general). `rho0` / `rho1`:
// the tension at the start / end of its run (< 0: the circle it is there,
// an end kept as it is or a face left round), going over evenly between.
// `fo`: one of its contacts on a cylinder made conic, followed: the contact
// moved onto its new surface, each conic tangent to it there. Null when it
// cannot.
static opencascade::handle<Geom_BSplineSurface> ten_general_surf(const TenFace& tf, double rho0, double rho1, TenFollow* fo) {
    BRepAdaptor_Surface sa(tf.f, Standard_False);
    // the corners exactly at the face's vertices
    std::vector<gp_Pnt> corners;
    for (TopExp_Explorer ex(tf.f, TopAbs_VERTEX); ex.More(); ex.Next()) corners.push_back(BRep_Tool::Pnt(TopoDS::Vertex(ex.Current())));
    auto snap = [&](const gp_Pnt& p) {
        for (auto& c : corners)
            if (c.Distance(p) < 1e-4) return c;
        return p;
    };
    double s0 = tf.sec_u ? tf.v0 : tf.u0, s1 = tf.sec_u ? tf.v1 : tf.u1;
    const int N = 32;
    std::vector<gp_Pnt> pa(N + 1), pb(N + 1);
    if (fo) fo->uv.assign(N + 1, gp_Pnt2d());
    std::vector<gp_Vec> ta(N + 1), tb(N + 1);
    // the faces' normals at the contacts (the conic's tangents stay square to them)
    std::vector<gp_Vec> na(N + 1), nb(N + 1);
    for (int j = 0; j <= N; j++) {
        double t = s0 + (s1 - s0) * j / N;
        gp_Vec x;
        if (tf.sec_u) {
            sa.D1(tf.u0, t, pa[j], ta[j], x);
            na[j] = ta[j].Crossed(x);
            sa.D1(tf.u1, t, pb[j], tb[j], x);
            nb[j] = tb[j].Crossed(x);
        } else {
            sa.D1(t, tf.v0, pa[j], x, ta[j]);
            na[j] = x.Crossed(ta[j]);
            sa.D1(t, tf.v1, pb[j], x, tb[j]);
            nb[j] = x.Crossed(tb[j]);
        }
        if (j == 0 || j == N) {
            pa[j] = snap(pa[j]);
            pb[j] = snap(pb[j]);
        }
        if (fo) {
            // the contact on the cylinder moved onto its new surface; the
            // conic in the plane through both contacts and the other face's
            // normal, tangent to that face and to the new surface
            gp_Pnt& pf = fo->side ? pb[j] : pa[j];
            gp_Pnt& po = fo->side ? pa[j] : pb[j];
            gp_Vec& tf_ = fo->side ? tb[j] : ta[j];
            gp_Vec& to = fo->side ? ta[j] : tb[j];
            gp_Vec nn;
            gp_Pnt np = ten_follow_point(*fo, pf, &nn, &fo->uv[j]);
            if (j == 0 || j == N) np = snap(np);
            // the other face's normal there
            gp_Vec no;
            {
                BRepAdaptor_Surface fa(fo->fixed, Standard_False);
                if (fa.GetType() == GeomAbs_Plane) no = gp_Vec(fa.Plane().Axis().Direction());
                else {
                    opencascade::handle<Geom_Surface> fs = BRep_Tool::Surface(fo->fixed);
                    GeomAPI_ProjectPointOnSurf pj(po, fs);
                    if (pj.NbPoints() == 0) return nullptr;
                    double pu, pv;
                    pj.LowerDistanceParameters(pu, pv);
                    gp_Pnt q;
                    gp_Vec du, dv;
                    fs->D1(pu, pv, q, du, dv);
                    no = du.Crossed(dv);
                }
            }
            gp_Vec m = no.Crossed(gp_Vec(po, np));
            if (m.Magnitude() < 1e-12 || nn.Magnitude() < 1e-12) return nullptr;
            gp_Vec t_o = m.Crossed(no), t_f = m.Crossed(nn);
            if (t_o.Magnitude() < 1e-12 || t_f.Magnitude() < 1e-12) return nullptr;
            // (both pointing on from the first contact to the second)
            gp_Vec ab = fo->side ? gp_Vec(po, np) : gp_Vec(np, po);
            if (t_o.Dot(ab) < 0) t_o.Reverse();
            if (t_f.Dot(ab) < 0) t_f.Reverse();
            to = t_o;
            tf_ = t_f;
            pf = np;
            (fo->side ? nb : na)[j] = nn;
            (fo->side ? na : nb)[j] = no;
        }
    }
    // an end shared with a cylinder made conic: its conic there exactly (the
    // tangents through its middle pole). When that section was a little off
    // its plane, the tangents there turn a little: the turn spread over the
    // run (smoothly, none at the other end), each tangent kept square to its
    // face's normal, so the rows do not wrinkle next to the end
    for (int end = 0; end < 2; end++) {
        const opencascade::handle<Geom_BezierCurve>& c = tf.endc[end];
        if (c.IsNull()) continue;
        int j = end ? N : 0;
        gp_Pnt c0 = c->Pole(1), k = c->Pole(2), c1 = c->Pole(3);
        double fwd = c0.Distance(pa[j]) + c1.Distance(pb[j]), back = c1.Distance(pa[j]) + c0.Distance(pb[j]);
        if (std::min(fwd, back) > 1e-5) return nullptr;
        if (back < fwd) std::swap(c0, c1);
        pa[j] = c0;
        pb[j] = c1;
        gp_Vec nta(c0, k), ntb(k, c1);
        if (nta.Magnitude() < 1e-12 || ntb.Magnitude() < 1e-12 || ta[j].Magnitude() < 1e-12 || tb[j].Magnitude() < 1e-12) return nullptr;
        std::vector<gp_Vec>* tt[2] = {&ta, &tb};
        std::vector<gp_Vec>* nn[2] = {&na, &nb};
        gp_Vec nt[2] = {nta, ntb};
        for (int side = 0; side < 2; side++) {
            std::vector<gp_Vec>& tv = *tt[side];
            gp_Vec un = nt[side].Normalized(), uo = tv[j].Normalized();
            double sg = uo.Dot(un) < 0 ? -1.0 : 1.0;
            gp_Vec d = un - uo * sg;
            if (d.Magnitude() > 1e-9)
                for (int i = 1; i < N; i++) {
                    if (tv[i].Magnitude() < 1e-12) continue;
                    double x = end ? double(i) / N : 1.0 - double(i) / N;
                    double f = x * x * (3.0 - 2.0 * x);
                    gp_Vec u = tv[i].Normalized() * sg + d * f;
                    gp_Vec n = (*nn[side])[i];
                    if (n.Magnitude() > 1e-9) {
                        n.Normalize();
                        u -= n * u.Dot(n);
                    }
                    if (u.Magnitude() > 1e-9) tv[i] = u;
                }
            tv[j] = nt[side];
        }
    }
    // a contact shrunk to a point: exactly that point all along
    for (std::vector<gp_Pnt>* side : {&pa, &pb}) {
        bool point = true;
        for (int j = 1; j <= N && point; j++) point = (*side)[j].Distance((*side)[0]) < 1e-6;
        if (point)
            for (auto& p : *side) p = (*side)[0];
    }
    // a circle's weight at an end: sin of half the angle at its middle pole
    auto circle_w = [&](int j) -> double {
        opencascade::handle<Geom_BezierCurve> c = ten_conic(pa[j], ta[j], pb[j], tb[j], 1.0);
        if (c.IsNull()) return -1.0;
        gp_Vec ka(c->Pole(2), pa[j]), kb(c->Pole(2), pb[j]);
        if (ka.Magnitude() < 1e-12 || kb.Magnitude() < 1e-12) return -1.0;
        return std::sin(0.5 * ka.Angle(kb));
    };
    double r0 = rho0, r1 = rho1;
    for (int end = 0; end < 2; end++) {
        double& r = end ? r1 : r0;
        if (r >= 0) continue;
        double wc = circle_w(end ? N : 0);
        if (wc <= 0) return nullptr;
        r = wc / (1.0 + wc);
    }
    r0 = std::min(0.98, std::max(0.02, r0));
    r1 = std::min(0.98, std::max(0.02, r1));
    bool even = std::fabs(r0 - r1) < 1e-12;
    double w = r0 / (1.0 - r0);
    opencascade::handle<TColgp_HArray1OfPnt> rows[3];
    for (auto& r : rows) r = new TColgp_HArray1OfPnt(1, N + 1);
    opencascade::handle<TColStd_HArray1OfReal> par = new TColStd_HArray1OfReal(1, N + 1);
    std::vector<double> ws(N + 1, w);
    for (int j = 0; j <= N; j++) {
        // the tension (rho) going over evenly from one end's to the other's
        double rj = r0 + (r1 - r0) * j / N;
        ws[j] = rj / (1.0 - rj);
        opencascade::handle<Geom_BezierCurve> c = ten_conic(pa[j], ta[j], pb[j], tb[j], ws[j]);
        if (c.IsNull()) return nullptr;
        rows[0]->SetValue(j + 1, pa[j]);
        rows[1]->SetValue(j + 1, c->Pole(2));
        rows[2]->SetValue(j + 1, pb[j]);
        par->SetValue(j + 1, s0 + (s1 - s0) * j / N);
    }
    opencascade::handle<Geom_BSplineCurve> cr[3];
    opencascade::handle<Geom_BSplineSurface> g = ten_rows(rows, par, w, cr, even ? nullptr : &ws);
    if (g.IsNull()) return nullptr;
    // its normal the way the old one's goes (the face keeps its orientation)
    gp_Pnt op, np;
    gp_Vec ou, ov, nu, nv;
    sa.D1(0.5 * (tf.u0 + tf.u1), 0.5 * (tf.v0 + tf.v1), op, ou, ov);
    g->D1(0.5, 0.5 * (s0 + s1), np, nu, nv);
    if (ou.Crossed(ov).Dot(nu.Crossed(nv)) < 0) g->UReverse();
    return g;
}

FShape* focc_fillet_tension(const FShape* s, const long long* tags, const double* rhos, int n, int* done, int* total, double* tight) {
    return guard(
        [&]() -> FShape* {
            for (int k = 0; k < n; k++) done[k] = total[k] = 0;
            for (int k = 0; k < 4 * n; k++) tight[k] = 0.0;
            if (n <= 0) {
                set_err("tension: no fillet");
                return nullptr;
            }
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            TopTools_IndexedDataMapOfShapeListOfShape e2f;
            TopExp::MapShapesAndAncestors(s->s, TopAbs_EDGE, TopAbs_FACE, e2f);
            TopTools_IndexedDataMapOfShapeListOfShape v2e;
            TopExp::MapShapesAndAncestors(s->s, TopAbs_VERTEX, TopAbs_EDGE, v2e);
            // the fillet's faces that can be made conic, as far as they go
            std::vector<TenFace> cand;
            // its sphere corners (three fillets meeting at a corner), and
            // which fillet each belongs to
            std::vector<std::pair<TopoDS_Face, int>> spheres;
            for (int i = 1; i <= fm.Extent(); i++) {
                long long t = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                int which = -1;
                for (int k = 0; k < n && which < 0; k++)
                    if (tags[k] == t) which = k;
                if (which < 0) continue;
                total[which]++;
                TenFace tf;
                tf.f = TopoDS::Face(fm(i));
                tf.tag = t;
                tf.which = which;
                tf.rho = std::min(0.98, std::max(0.02, rhos[which]));
                double w = tf.rho / (1.0 - tf.rho);
                BRepAdaptor_Surface sa(tf.f, Standard_False);
                if (sa.GetType() == GeomAbs_Cylinder) {
                    tf.torus = false;
                    tf.frame = sa.Cylinder().Position();
                } else if (sa.GetType() == GeomAbs_Torus) {
                    tf.torus = true;
                    tf.frame = sa.Torus().Position();
                } else if (sa.GetType() == GeomAbs_Sphere) {
                    spheres.push_back({tf.f, which});
                    continue;
                } else {
                    // a fillet along another curve: made conic place by place
                    auto own = [&](const TopoDS_Edge& e) {
                        if (!e2f.Contains(e)) return false;
                        for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next()) {
                            if (it.Value().IsSame(tf.f)) continue;
                            int j = fm.FindIndex(it.Value());
                            if (j > 0 && j - 1 < (int)s->tags.size() && s->tags[j - 1] == t) return true;
                        }
                        return false;
                    };
                    if (sa.GetType() != GeomAbs_Plane && ten_general(tf, w, own)) cand.push_back(tf);
                    else if (getenv("FORME_OCCT_DEBUG")) {
                        int ne = 0;
                        for (TopExp_Explorer ex(tf.f, TopAbs_EDGE); ex.More(); ex.Next()) ne++;
                        fprintf(stderr, "tension: face of surface type %d (%d edges) not shaped as a fillet\n", (int)sa.GetType(), ne);
                    }
                    continue;
                }
                BRepTools::UVBounds(tf.f, tf.u0, tf.u1, tf.v0, tf.v1);
                gp_Dir z = tf.frame.Direction();
                bool ok = true;
                for (TopExp_Explorer ex(tf.f, TopAbs_EDGE); ex.More() && ok; ex.Next()) {
                    TopoDS_Edge e = TopoDS::Edge(ex.Current());
                    if (BRep_Tool::Degenerated(e)) {
                        ok = false;
                        break;
                    }
                    bool seen = false;
                    for (auto& x : tf.contacts) seen |= x.IsSame(e);
                    for (auto& x : tf.sections) seen |= x.IsSame(e);
                    if (seen) continue;
                    // along the fillet: constant in the section's parameter
                    double f0, f1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                    if (pc.IsNull()) {
                        ok = false;
                        break;
                    }
                    gp_Pnt2d p0 = pc->Value(f0), p1 = pc->Value(f1), pm = pc->Value(0.5 * (f0 + f1));
                    // a cylinder's section runs in u, a torus's in v
                    double a0 = tf.torus ? p0.Y() : p0.X(), a1 = tf.torus ? p1.Y() : p1.X(), am = tf.torus ? pm.Y() : pm.X();
                    bool along = std::fabs(a0 - a1) < 1e-7 && std::fabs(a0 - am) < 1e-7;
                    if (along) tf.contacts.push_back(e);
                    else tf.sections.push_back(e);
                    (void)z;
                }
                if (!ok || tf.contacts.size() != 2) continue;
                // the conic at the start of the sweep (u0 for a torus, v0 for a cylinder)
                gp_Pnt a, b;
                gp_Vec du, dv;
                if (tf.torus) {
                    sa.D1(tf.u0, tf.v0, a, du, dv);
                    gp_Vec ta = dv;
                    sa.D1(tf.u0, tf.v1, b, du, dv);
                    tf.gen = ten_conic(a, ta, b, dv, w);
                } else {
                    sa.D1(tf.u0, tf.v0, a, du, dv);
                    gp_Vec ta = du;
                    sa.D1(tf.u1, tf.v0, b, du, dv);
                    tf.gen = ten_conic(a, ta, b, du, w);
                }
                if (tf.gen.IsNull()) continue;
                cand.push_back(tf);
            }
            // the faces of fillets not asked that run along a cylinder made
            // conic (a corner OpenCascade made between two fillets): they
            // follow it
            size_t asked_cands = cand.size();
            for (int i = 1; i <= fm.Extent(); i++) {
                long long t = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                bool is_asked = false;
                for (int k = 0; k < n; k++) is_asked |= tags[k] == t;
                if (is_asked) continue;
                TopoDS_Face f = TopoDS::Face(fm(i));
                GeomAbs_SurfaceType ty = BRepAdaptor_Surface(f, Standard_False).GetType();
                if (ty != GeomAbs_BSplineSurface && ty != GeomAbs_BezierSurface && ty != GeomAbs_OtherSurface) continue;
                bool next = false;
                for (TopExp_Explorer ex(f, TopAbs_EDGE); ex.More() && !next; ex.Next())
                    for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(ex.Current())); it.More(); it.Next())
                        for (size_t k = 0; k < asked_cands; k++)
                            next |= !it.Value().IsSame(f) && cand[k].f.IsSame(it.Value()) && !cand[k].general && !cand[k].torus;
                if (!next) continue;
                TenFace tf;
                tf.f = f;
                tf.tag = t;
                tf.which = -1;
                tf.follower = true;
                tf.rho = -1.0;
                auto own = [&](const TopoDS_Edge& e) {
                    for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next()) {
                        if (it.Value().IsSame(f)) continue;
                        int j = fm.FindIndex(it.Value());
                        if (j > 0 && j - 1 < (int)s->tags.size() && s->tags[j - 1] == t) return true;
                    }
                    return false;
                };
                if (ten_general(tf, std::sqrt(0.5), own)) cand.push_back(tf);
            }
            // drop the faces whose ends are neither flat nor a face kept here,
            // again until none goes (a run of fillets goes together)
            auto in_cand = [&](const TopoDS_Shape& f) -> const TenFace* {
                for (auto& c : cand)
                    if (c.f.IsSame(f)) return &c;
                return nullptr;
            };
            // a torus where one fillet runs round another's rounded edge (the
            // top edges' fillet round a vertical edge's): between that edge's
            // cylinder (round the same axis) and a flat face square to the
            // axis, its two ends on the first fillet's cylinders. Its section
            // takes the tension of the fillet whose ends it joins (a circle
            // when none), its run follows the cylinder's new section (a conic
            // when that fillet has tension), whichever fillet it belongs to.
            struct TenCorner {
                TopoDS_Face f, cy, pl, x[2];
                TopoDS_Edge base, top, sec[2];
                int which;  // its fillet among those asked, else -1
                gp_Pnt o;
                gp_Dir a;
                double h, sr;  // from its base to its top: along the axis, out from it
            };
            std::vector<TenCorner> corners;
            const double w_circle = std::sqrt(0.5);  // a quarter circle's
            auto rho_w = [](double rho) {
                rho = std::min(0.98, std::max(0.02, rho));
                return rho / (1.0 - rho);
            };
            auto asked = [&](const TopoDS_Shape& f) -> int {
                int i = fm.FindIndex(f);
                long long t = i >= 1 && i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                for (int k = 0; k < n; k++)
                    if (tags[k] == t) return k;
                return -1;
            };
            auto other = [&](const TopoDS_Edge& e, const TopoDS_Shape& f) -> TopoDS_Face {
                for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next())
                    if (!it.Value().IsSame(f)) return TopoDS::Face(it.Value());
                return TopoDS_Face();
            };
            for (int i = 1; i <= fm.Extent(); i++) {
                TopoDS_Face f = TopoDS::Face(fm(i));
                BRepAdaptor_Surface sa(f, Standard_False);
                if (sa.GetType() != GeomAbs_Torus) continue;
                TenCorner c;
                c.f = f;
                c.which = asked(f);
                c.o = sa.Torus().Location();
                c.a = sa.Torus().Axis().Direction();
                std::vector<TopoDS_Edge> along, across;
                bool ok = true;
                for (TopExp_Explorer ex(f, TopAbs_EDGE); ex.More() && ok; ex.Next()) {
                    TopoDS_Edge e = TopoDS::Edge(ex.Current());
                    if (BRep_Tool::Degenerated(e) || BRep_Tool::IsClosed(e, f)) {
                        ok = false;
                        break;
                    }
                    bool seen = false;
                    for (auto& x : along) seen |= x.IsSame(e);
                    for (auto& x : across) seen |= x.IsSame(e);
                    if (seen) continue;
                    double f0, f1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, f, f0, f1);
                    if (pc.IsNull()) {
                        ok = false;
                        break;
                    }
                    gp_Pnt2d p0 = pc->Value(f0), p1 = pc->Value(f1), pm = pc->Value(0.5 * (f0 + f1));
                    bool cv = std::fabs(p0.Y() - p1.Y()) < 1e-7 && std::fabs(p0.Y() - pm.Y()) < 1e-7;
                    bool cu = std::fabs(p0.X() - p1.X()) < 1e-7 && std::fabs(p0.X() - pm.X()) < 1e-7;
                    if (cv == cu) {
                        ok = false;
                        break;
                    }
                    (cv ? along : across).push_back(e);
                }
                if (!ok || along.size() != 2 || across.size() != 2) continue;
                int nb = 0, nt = 0;
                for (auto& e : along) {
                    TopoDS_Face g = other(e, f);
                    if (g.IsNull()) continue;
                    BRepAdaptor_Surface ga(g, Standard_False);
                    if (ga.GetType() == GeomAbs_Cylinder && ga.Cylinder().Axis().Direction().IsParallel(c.a, 1e-6) && gp_Lin(ga.Cylinder().Axis()).Distance(c.o) < 1e-6) {
                        c.base = e;
                        c.cy = g;
                        nb++;
                    } else if (ga.GetType() == GeomAbs_Plane && ga.Plane().Axis().Direction().IsParallel(c.a, 1e-6)) {
                        c.top = e;
                        c.pl = g;
                        nt++;
                    }
                }
                if (nb != 1 || nt != 1) continue;
                for (int k = 0; k < 2 && ok; k++) {
                    c.sec[k] = across[k];
                    c.x[k] = other(across[k], f);
                    ok = !c.x[k].IsNull() && BRepAdaptor_Surface(c.x[k], Standard_False).GetType() == GeomAbs_Cylinder;
                }
                if (!ok) continue;
                auto on = [](const TopoDS_Edge& e, const TopoDS_Vertex& v) {
                    TopoDS_Vertex a, b;
                    TopExp::Vertices(e, a, b);
                    return a.IsSame(v) || b.IsSame(v);
                };
                TopoDS_Vertex s1, s2;
                TopExp::Vertices(c.sec[0], s1, s2);
                TopoDS_Vertex vb = on(c.base, s1) ? s1 : s2, vt = on(c.top, s1) ? s1 : s2;
                if (vb.IsSame(vt) || !on(c.base, vb) || !on(c.top, vt)) continue;
                gp_Pnt pb = BRep_Tool::Pnt(vb), pt = BRep_Tool::Pnt(vt);
                gp_Lin ax(c.o, c.a);
                c.h = gp_Vec(pb, pt).Dot(gp_Vec(c.a));
                c.sr = ax.Distance(pt) - ax.Distance(pb);
                // only where a plain torus fillet would not do: the cylinder
                // made conic, the torus not asked for, or its ends' tension
                // not its own
                int wc = asked(c.cy), w0 = asked(c.x[0]), w1 = asked(c.x[1]);
                if (c.which < 0 && wc < 0 && w0 < 0 && w1 < 0) continue;
                bool need = wc >= 0 || c.which < 0;
                for (int x : {w0, w1})
                    if (!need && c.which >= 0) need = std::fabs((x < 0 ? w_circle : rho_w(rhos[x])) - rho_w(rhos[c.which])) > 1e-9;
                if (!need) continue;
                corners.push_back(c);
                for (size_t k = 0; k < cand.size(); k++)
                    if (cand[k].f.IsSame(f)) {
                        cand.erase(cand.begin() + k);
                        break;
                    }
            }
            auto corner_of = [&](const TopoDS_Shape& f) -> const TenCorner* {
                for (auto& c : corners)
                    if (c.f.IsSame(f)) return &c;
                return nullptr;
            };
            // a corner's section weight (its two ends the same), false when they differ
            auto corner_w = [&](const TenCorner& c, double& w) {
                double ws[2];
                for (int k = 0; k < 2; k++) {
                    const TenFace* o = in_cand(c.x[k]);
                    ws[k] = o ? o->rho / (1.0 - o->rho) : w_circle;
                }
                w = ws[0];
                return std::fabs(ws[0] - ws[1]) < 1e-9;
            };
            // a fillet running round a cylinder made conic needs the conic
            // no tighter than itself (else its edge on the flat face would
            // loop back; a tenth of margin): when it is, the cylinder stays
            // round, and
            // `tight` gets its tightest radius, the size needed, and the
            // tensions that would do
            auto corner_tight = [&](const TenCorner& c) {
                const TenFace* o = in_cand(c.cy);
                if (!o || o->gen.IsNull() || o->gen->NbPoles() != 3) return false;
                gp_Pnt a = o->gen->Pole(1), k = o->gen->Pole(2), b = o->gen->Pole(3);
                // (a tenth more: its edge on the flat face keeps a little roundness)
                double need = std::fabs(c.sr), want = 1.1 * need;
                double r = conic_min_radius(a, k, b, o->gen->Weight(2));
                if (r >= want - 1e-6) return false;
                double* t = tight + 4 * o->which;
                if (t[1] == 0.0) {
                    t[0] = r;
                    t[1] = need;
                    for (int i = 4; i <= 196; i++) {
                        double rho = i / 200.0;
                        if (conic_min_radius(a, k, b, rho / (1.0 - rho)) >= want) {
                            if (t[2] == 0.0) t[2] = rho;
                            t[3] = rho;
                        }
                    }
                }
                return true;
            };
            // a sphere corner where three straight fillets made conic with
            // the same tension meet: it is remade to fit them
            auto corner_ok = [&](const TopoDS_Shape& f, double rho) {
                bool is = false;
                for (auto& c : spheres) is |= c.first.IsSame(f);
                if (!is) return false;
                int n = 0;
                for (TopExp_Explorer ex(f, TopAbs_EDGE); ex.More(); ex.Next()) {
                    TopoDS_Edge e = TopoDS::Edge(ex.Current());
                    if (BRep_Tool::Degenerated(e)) continue;  // its pole
                    n++;
                    bool found = false;
                    for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next()) {
                        if (it.Value().IsSame(f)) continue;
                        const TenFace* o = in_cand(it.Value());
                        if (!o || o->torus || o->general || std::fabs(o->rho - rho) > 1e-9) return false;
                        found = true;
                    }
                    if (!found) return false;
                }
                return n == 3;
            };
            // a general face's section kept as it is: a circle shared with a
            // round face that stays so (not made conic here, not a corner)
            auto general_keeps = [&](const TenFace& tf, const TopoDS_Edge& e) {
                if (!edge_is_arc(e)) return false;
                for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next()) {
                    if (it.Value().IsSame(tf.f)) continue;
                    if (in_cand(it.Value()) || corner_of(it.Value())) return false;
                    GeomAbs_SurfaceType k = BRepAdaptor_Surface(TopoDS::Face(it.Value()), Standard_False).GetType();
                    if (k != GeomAbs_Cylinder && k != GeomAbs_Torus) return false;
                }
                return true;
            };
            // the plane a section lies on: the flat face at that end, else
            // (a section shared with another face made conic) its own plane:
            // square to a cylinder's axis, or where two fillets meet in a
            // mitre any plane it lies on; a torus's sections are meridians
            auto section_plane = [&](const TenFace& tf, const TopoDS_Edge& e, gp_Dir& n, gp_Pnt& q) -> bool {
                if (tf.general) {
                    // shared with a face made conic with the same tension, on
                    // a flat face, or a circle on a round face left so (kept)
                    bool any = false;
                    for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next()) {
                        if (it.Value().IsSame(tf.f)) continue;
                        if (const TenFace* o = in_cand(it.Value())) {
                            // a cylinder's or torus's end: its tension there;
                            // another general face's: the same tension
                            if (o->general && std::fabs(o->rho - tf.rho) > 1e-9) return false;
                        } else if (BRepAdaptor_Surface(TopoDS::Face(it.Value()), Standard_False).GetType() != GeomAbs_Plane) {
                            if (!general_keeps(tf, e)) return false;
                        }
                        any = true;
                    }
                    return any;
                }
                gp_Dir z = tf.frame.Direction();
                auto fits = [&]() {
                    if (tf.torus) return std::fabs(n.Dot(z)) < 1e-6 && gp_Pln(q, n).Distance(tf.frame.Location()) < 1e-6;
                    return std::fabs(n.Dot(z)) >= 0.05;
                };
                if (BRep_Tool::IsClosed(e, tf.f)) return tf.torus;  // a full torus's seam
                const TopTools_ListOfShape& fs = e2f.FindFromKey(e);
                bool shared = false;
                bool general_end = false;  // shared with a general face's end only
                for (TopTools_ListOfShape::Iterator it(fs); it.More(); it.Next()) {
                    if (it.Value().IsSame(tf.f)) continue;
                    if (const TenCorner* c = corner_of(it.Value())) {
                        // a corner round this cylinder's end, or joining this fillet's end
                        double w;
                        if (!corner_w(*c, w)) return false;
                        bool mine = (e.IsSame(c->base) && tf.f.IsSame(c->cy)) || (e.IsSame(c->sec[0]) && tf.f.IsSame(c->x[0])) || (e.IsSame(c->sec[1]) && tf.f.IsSame(c->x[1]));
                        if (!mine || (e.IsSame(c->base) && corner_tight(*c))) return false;
                        shared = true;
                        continue;
                    }
                    if (const TenFace* o = in_cand(it.Value())) {
                        if (o->general) {
                            // a general face's contact following this one:
                            // the edge it makes; its end: it takes this conic
                            for (auto& x : o->contacts)
                                if (x.IsSame(e)) return !tf.torus;
                            shared = true;
                            general_end = !tf.torus && std::fabs(o->rho - tf.rho) < 1e-9;
                            continue;
                        }
                        // its conic must be this one's there: the same tension
                        if (std::fabs(o->rho - tf.rho) > 1e-9) return false;
                        shared = true;
                        continue;
                    }
                    if (!tf.torus && corner_ok(it.Value(), tf.rho)) {
                        shared = true;
                        continue;
                    }
                    BRepAdaptor_Surface ea(TopoDS::Face(it.Value()), Standard_False);
                    if (ea.GetType() != GeomAbs_Plane) return false;
                    n = ea.Plane().Axis().Direction();
                    q = ea.Plane().Location();
                    return fits();
                }
                if (!shared) return false;
                BRepAdaptor_Curve c(e);
                double f0 = c.FirstParameter(), f1 = c.LastParameter();
                gp_Pnt a = c.Value(f0), b = c.Value(0.5 * (f0 + f1)), d = c.Value(f1);
                gp_Vec nn = gp_Vec(a, b).Crossed(gp_Vec(a, d));
                if (nn.Magnitude() < 1e-12) return false;
                n = gp_Dir(nn);
                q = a;
                // (where a fillet's end meets a corner OpenCascade made of it,
                // a curve a little off its plane: the plane through its ends
                // and middle, the general face's end row made on it too)
                double off = 0;
                for (int k = 1; k < 16; k++) off = std::max(off, std::fabs(gp_Vec(q, c.Value(f0 + (f1 - f0) * k / 16.0)).Dot(gp_Vec(n))));
                if (off > 1e-6 && !(general_end && off < 0.01 * a.Distance(d))) {
                    if (getenv("FORME_OCCT_DEBUG")) fprintf(stderr, "tension: section of face %d not flat (%g off)\n", fm.FindIndex(tf.f), off);
                    return false;
                }
                return fits();
            };
            // a general face's contacts: on faces left as they are, or one
            // on a cylinder made conic, which it follows (a follower must)
            auto contacts_ok = [&](TenFace& tf) {
                tf.follow = -1;
                for (size_t i = 0; i < tf.contacts.size(); i++)
                    for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(tf.contacts[i])); it.More(); it.Next()) {
                        if (it.Value().IsSame(tf.f)) continue;
                        if (const TenFace* o = in_cand(it.Value())) {
                            if (o->general || o->torus || tf.follow >= 0) return false;
                            tf.follow = (int)i;
                        } else if (corner_of(it.Value())) {
                            return false;
                        }
                    }
                return !tf.follower || tf.follow >= 0;
            };
            auto end_ok = [&](const TenFace& tf, const TopoDS_Edge& e) {
                gp_Dir n;
                gp_Pnt q;
                return section_plane(tf, e, n, q);
            };
            for (bool again = true; again;) {
                for (bool changed = true; changed;) {
                    changed = false;
                    for (size_t k = 0; k < cand.size(); k++) {
                        bool ok = true;
                        for (auto& e : cand[k].sections) ok = ok && end_ok(cand[k], e);
                        if (ok && cand[k].general) ok = contacts_ok(cand[k]);
                        if (!ok && getenv("FORME_OCCT_DEBUG")) fprintf(stderr, "tension: face %d (fillet %d, %s) dropped: its ends or contacts\n", fm.FindIndex(cand[k].f), cand[k].which, cand[k].general ? "general" : cand[k].torus ? "torus" : "cylinder");
                        if (!ok) {
                            cand.erase(cand.begin() + k);
                            changed = true;
                            break;
                        }
                    }
                }
                // a general face's ends: a cylinder's or torus's conic there
                // (its tension), a kept circle, else its own tension (a
                // follower's: round), going over evenly between; a contact
                // on a cylinder made conic followed
                again = false;
                for (size_t k = 0; k < cand.size() && !again; k++) {
                    TenFace& tf = cand[k];
                    if (!tf.general) continue;
                    double rend[2] = {tf.rho, tf.rho};
                    double s0 = tf.sec_u ? tf.v0 : tf.u0, s1 = tf.sec_u ? tf.v1 : tf.u1;
                    for (size_t i = 0; i < tf.sections.size(); i++) {
                        const TopoDS_Edge& e = tf.sections[i];
                        tf.kept[i] = false;
                        const TenFace* o = nullptr;
                        for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(e)); it.More(); it.Next())
                            if (!it.Value().IsSame(tf.f) && in_cand(it.Value())) o = in_cand(it.Value());
                        bool kp = !o && general_keeps(tf, e);
                        tf.kept[i] = kp;
                        double f0, f1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                        gp_Pnt2d pm = pc->Value(0.5 * (f0 + f1));
                        double at = tf.sec_u ? pm.Y() : pm.X();
                        int end = std::fabs(at - s0) < std::fabs(at - s1) ? 0 : 1;
                        if (kp) rend[end] = -1.0;
                        else if (o) rend[end] = o->rho;
                        tf.endc[end].Nullify();
                        if (o && !o->general && !o->torus) {
                            // the cylinder's conic moved onto the section's plane
                            gp_Dir z = o->frame.Direction(), n = z;
                            gp_Pnt q = o->gen->StartPoint();
                            if (section_plane(*o, e, n, q)) {
                                double nz = gp_Vec(n).Dot(gp_Vec(z));
                                gp_Vec vz(z), vn(n);
                                if (std::fabs(nz) > 1e-9) tf.endc[end] = ten_map(o->gen, [=](const gp_Pnt& p) { return p.Translated(vz * (vn.Dot(gp_Vec(p, q)) / nz)); }, false);
                            }
                        }
                    }
                    TenFollow fo;
                    if (tf.follow >= 0) {
                        const TopoDS_Edge& c = tf.contacts[tf.follow];
                        const TenFace* x = nullptr;
                        for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(c)); it.More(); it.Next())
                            if (!it.Value().IsSame(tf.f) && in_cand(it.Value())) x = in_cand(it.Value());
                        double f0, f1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(c, tf.f, f0, f1);
                        gp_Pnt2d pm = pc->Value(0.5 * (f0 + f1));
                        fo.side = tf.sec_u ? (std::fabs(pm.X() - tf.u0) < std::fabs(pm.X() - tf.u1) ? 0 : 1) : (std::fabs(pm.Y() - tf.v0) < std::fabs(pm.Y() - tf.v1) ? 0 : 1);
                        for (size_t i = 0; i < tf.contacts.size(); i++)
                            if ((int)i != tf.follow)
                                for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(tf.contacts[i])); it.More(); it.Next())
                                    if (!it.Value().IsSame(tf.f)) fo.fixed = TopoDS::Face(it.Value());
                        fo.frame = x->frame;
                        fo.v0 = x->v0;
                        fo.gen = x->gen;
                    }
                    bool cannot = tf.follow >= 0 && fo.fixed.IsNull();  // (no face across)
                    bool uniform = !tf.follower && tf.follow < 0 && std::fabs(rend[0] - tf.rho) < 1e-12 && std::fabs(rend[1] - tf.rho) < 1e-12 && tf.endc[0].IsNull() && tf.endc[1].IsNull();
                    if (uniform) continue;  // the surface made at first holds
                    opencascade::handle<Geom_BSplineSurface> g;
                    // (not both ends kept round with nothing followed: it would not change)
                    if (!cannot && !(rend[0] < 0 && rend[1] < 0 && tf.follow < 0)) g = ten_general_surf(tf, rend[0], rend[1], tf.follow >= 0 ? &fo : nullptr);
                    if (!g.IsNull() && tf.follow >= 0) {
                        // the followed contact's new edge: the new surface's own row
                        const TopoDS_Edge& c = tf.contacts[tf.follow];
                        TopoDS_Vertex c1, c2;
                        TopExp::Vertices(c, c1, c2);
                        opencascade::handle<Geom_Curve> iso[2] = {g->UIso(0.0), g->UIso(1.0)};
                        gp_Pnt p1 = BRep_Tool::Pnt(c1), p2 = BRep_Tool::Pnt(c2);
                        double d[2];
                        for (int r = 0; r < 2; r++) {
                            gp_Pnt e0 = iso[r]->Value(s0), e1 = iso[r]->Value(s1);
                            d[r] = std::min(e0.Distance(p1) + e1.Distance(p2), e0.Distance(p2) + e1.Distance(p1));
                        }
                        int r = d[0] < d[1] ? 0 : 1;
                        gp_Pnt e0 = iso[r]->Value(s0);
                        bool fwd = e0.Distance(p1) < e0.Distance(p2);
                        if (d[r] > 1e-4) g.Nullify();
                        else {
                            BRepBuilderAPI_MakeEdge me(iso[r], fwd ? c1 : c2, fwd ? c2 : c1, s0, s1);
                            // its curve on the cylinder's new surface: through
                            // the samples' places there, run the same way
                            const int M = (int)fo.uv.size();
                            opencascade::handle<TColgp_HArray1OfPnt2d> q2 = new TColgp_HArray1OfPnt2d(1, M);
                            opencascade::handle<TColStd_HArray1OfReal> qp = new TColStd_HArray1OfReal(1, M);
                            for (int j = 0; j < M; j++) {
                                q2->SetValue(j + 1, fo.uv[j]);
                                qp->SetValue(j + 1, s0 + (s1 - s0) * j / (M - 1));
                            }
                            Geom2dAPI_Interpolate ip2(q2, qp, Standard_False, 1e-9);
                            ip2.Perform();
                            if (me.IsDone() && ip2.IsDone()) {
                                tf.follow_edge = me.Edge();
                                tf.follow_pc = ip2.Curve();
                                tf.follow_u = r;
                            } else {
                                g.Nullify();
                            }
                        }
                    }
                    if (g.IsNull()) {
                        cand.erase(cand.begin() + k);
                        again = true;
                    } else {
                        tf.gsurf = g;
                    }
                }
            }

            if (cand.empty() && corners.empty()) {
                set_err("tension: only on fillets along straight or circular edges, ending on flat faces or a fillet of the same size and tension");
                return nullptr;
            }
            opencascade::handle<ShapeBuild_ReShape> rs = new ShapeBuild_ReShape();
            TopTools_DataMapOfShapeShape made;  // section edge → its conic edge
            // (a general face's contact followed: its new edge)
            for (auto& tf : cand)
                if (tf.general && tf.follow >= 0) made.Bind(tf.contacts[tf.follow], tf.follow_edge);
            TopTools_DataMapOfShapeShape vmap;  // a vertex moved onto a new surface
            auto mv = [&](const TopoDS_Vertex& v) { return vmap.IsBound(v) ? TopoDS::Vertex(vmap.Find(v)) : v; };
            BRep_Builder bb;
            // a new face's wires: the old face's, the edges remade swapped in
            // (their curves on it by projection when they have none)
            auto rewire = [&](const TopoDS_Face& of, TopoDS_Face& nf) {
                TopoDS_Face ff = TopoDS::Face(of.Oriented(TopAbs_FORWARD));
                for (TopoDS_Iterator wi(ff); wi.More(); wi.Next()) {
                    TopoDS_Wire ow = TopoDS::Wire(wi.Value());
                    TopoDS_Wire nw;
                    bb.MakeWire(nw);
                    for (TopoDS_Iterator it(ow, Standard_False); it.More(); it.Next()) {
                        TopoDS_Edge e = TopoDS::Edge(it.Value());
                        TopoDS_Edge ne = made.IsBound(e) ? TopoDS::Edge(made.Find(e)) : e;
                        if (made.IsBound(e)) {
                            // the new edge may run the other way
                            TopoDS_Vertex a1, a2, b1, b2;
                            TopExp::Vertices(e, a1, a2, Standard_False);
                            TopExp::Vertices(TopoDS::Edge(ne), b1, b2, Standard_False);
                            bool flip = !mv(a1).IsSame(b1);
                            ne.Orientation(flip ? TopAbs::Reverse(e.Orientation()) : e.Orientation());
                        } else {
                            ne.Orientation(e.Orientation());
                        }
                        bb.Add(nw, ne);
                    }
                    nw.Orientation(ow.Orientation());
                    bb.Add(nf, nw);
                }
                for (TopExp_Explorer ex(nf, TopAbs_EDGE); ex.More(); ex.Next()) {
                    TopoDS_Edge e = TopoDS::Edge(ex.Current());
                    double g0, g1;
                    if (BRep_Tool::CurveOnSurface(e, nf, g0, g1).IsNull()) {
                        ShapeFix_Edge sfe;
                        sfe.FixAddPCurve(e, nf, Standard_False, 1e-6);
                    }
                }
                nf.Orientation(of.Orientation());
                BRepLib::SameParameter(nf, 1e-6);
            };
            // the edges kept (old circles, contacts) sit on a sampled new
            // surface within the samples' error: their tolerance made to cover it
            auto cover = [&](const TopoDS_Face& nf, const opencascade::handle<Geom_Surface>& g) {
                for (TopExp_Explorer ex(nf, TopAbs_EDGE); ex.More(); ex.Next()) {
                    TopoDS_Edge e = TopoDS::Edge(ex.Current());
                    if (BRep_Tool::Degenerated(e)) continue;
                    double g0, g1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, nf, g0, g1);
                    if (pc.IsNull()) continue;
                    BRepAdaptor_Curve ec(e);
                    double d = 0;
                    // (finer than the samples the surfaces were made through,
                    // and between them)
                    for (int k = 0; k <= 160; k++) {
                        double t = ec.FirstParameter() + (ec.LastParameter() - ec.FirstParameter()) * k / 160.0;
                        gp_Pnt2d uv = pc->Value(g0 + (g1 - g0) * k / 160.0);
                        d = std::max(d, g->Value(uv.X(), uv.Y()).Distance(ec.Value(t)));
                    }
                    if (d > BRep_Tool::Tolerance(e)) bb.UpdateEdge(e, 1.5 * d);
                    if (d > 1e-4 && getenv("FORME_OCCT_DEBUG")) fprintf(stderr, "tension: an edge kept %g off its new face\n", d);
                }
            };
            for (TenFace& tf : cand) {
                // a torus's u turns round X × Y (its frame may be left-handed)
                gp_Dir z = tf.torus ? tf.frame.XDirection().Crossed(tf.frame.YDirection()) : tf.frame.Direction();
                gp_Pnt o = tf.frame.Location();
                opencascade::handle<Geom_Surface> surf;
                if (tf.general) surf = tf.gsurf;
                else if (tf.torus) surf = new Geom_SurfaceOfRevolution(tf.gen, gp_Ax1(o, z));
                else surf = new Geom_SurfaceOfLinearExtrusion(tf.gen, z);
                TopoDS_Face nf;
                bb.MakeFace(nf, surf, 1e-7);
                bool ok = true;
                // along the fillet: the same edges, their (u, v) moved to (sweep, t)
                // (on a general face, projected below)
                for (const TopoDS_Edge& e : tf.contacts) {
                    if (tf.general) break;
                    double f0, f1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                    gp_Pnt2d pm = pc->Value(0.5 * (f0 + f1));
                    opencascade::handle<Geom2d_Curve> npc = opencascade::handle<Geom2d_Curve>::DownCast(pc->Copy());
                    if (tf.torus) {
                        double t = ten_near_mod(pm.Y(), tf.v0) ? 0.0 : 1.0;
                        npc->Translate(gp_Vec2d(-tf.u0, t - pm.Y()));
                    } else {
                        double t = ten_near_mod(pm.X(), tf.u0) ? 0.0 : 1.0;
                        npc->Translate(gp_Vec2d(t - pm.X(), -tf.v0));
                    }
                    if (BRep_Tool::IsClosed(e, tf.f)) {
                        ok = false;
                        break;
                    }
                    bb.UpdateEdge(e, npc, nf, 1e-6);
                }
                if (!ok) continue;
                std::vector<TopoDS_Shape> bound_e, bound_v;  // remade for this face
                // where two of its sections meet (its end across two flat
                // faces, a chamfer's and an end's): the vertex moved onto the
                // new surface along the edge that leaves the fillet there
                for (size_t a = 0; a < tf.sections.size() && ok && !tf.general; a++)
                    for (int end = 0; end < 2 && ok; end++) {
                        TopoDS_Vertex v = end ? TopExp::LastVertex(tf.sections[a]) : TopExp::FirstVertex(tf.sections[a]);
                        if (vmap.IsBound(v)) continue;
                        auto touches = [&](const std::vector<TopoDS_Edge>& es) {
                            int k = 0;
                            for (auto& x : es) {
                                TopoDS_Vertex x1, x2;
                                TopExp::Vertices(x, x1, x2);
                                k += (int)x1.IsSame(v) + (int)x2.IsSame(v);
                            }
                            return k;
                        };
                        if (touches(tf.sections) < 2 || touches(tf.contacts) > 0) continue;
                        std::vector<TopoDS_Edge> leave;
                        for (TopTools_ListOfShape::Iterator it(v2e.FindFromKey(v)); it.More(); it.Next()) {
                            bool seen = false;
                            for (auto& x : tf.sections) seen |= x.IsSame(it.Value());
                            for (auto& x : leave) seen |= x.IsSame(it.Value());
                            if (!seen) leave.push_back(TopoDS::Edge(it.Value()));
                        }
                        if (leave.size() != 1) {
                            ok = false;
                            break;
                        }
                        TopoDS_Edge te = leave[0];
                        // between flat faces only (no curve on them to remake)
                        for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(te)); it.More(); it.Next())
                            if (in_cand(it.Value()) || BRepAdaptor_Surface(TopoDS::Face(it.Value()), Standard_False).GetType() != GeomAbs_Plane) ok = false;
                        double c0, c1;
                        opencascade::handle<Geom_Curve> tc = BRep_Tool::Curve(te, c0, c1);
                        if (!ok || tc.IsNull()) {
                            ok = false;
                            break;
                        }
                        // the new point on its curve, on v's side of its other end
                        TopoDS_Vertex t1, t2;
                        TopExp::Vertices(te, t1, t2);
                        bool at_start = t1.IsSame(v);
                        gp_Pnt old = BRep_Tool::Pnt(v);
                        GeomAPI_IntCS ics(tc, surf);
                        int best = 0;
                        double bd = 1e100;
                        if (ics.IsDone())
                            for (int k = 1; k <= ics.NbPoints(); k++) {
                                double pu, pv, pt;
                                ics.Parameters(k, pu, pv, pt);
                                if (at_start ? pt > c1 - 1e-6 : pt < c0 + 1e-6) continue;
                                double d = ics.Point(k).Distance(old);
                                if (d < bd) {
                                    bd = d;
                                    best = k;
                                }
                            }
                        if (best == 0 || bd > 0.5 * tf.gen->StartPoint().Distance(tf.gen->EndPoint())) {
                            ok = false;
                            break;
                        }
                        TopoDS_Vertex nv = BRepBuilderAPI_MakeVertex(ics.Point(best));
                        vmap.Bind(v, nv);
                        bound_v.push_back(v);
                        BRepBuilderAPI_MakeEdge me(tc, mv(t1), mv(t2));
                        if (!me.IsDone()) {
                            ok = false;
                            break;
                        }
                        made.Bind(te, me.Edge());
                        bound_e.push_back(te);
                    }
                // the sections: the conic moved there
                bool followed_any = false;
                for (const TopoDS_Edge& e : tf.sections) {
                    if (!ok) break;
                    if (!tf.general) {
                        // a general face's contact following it: made there
                        bool followed = false;
                        for (auto& o : cand)
                            followed |= o.general && o.follow >= 0 && o.contacts[o.follow].IsSame(e);
                        if (followed) {
                            if (!made.IsBound(e)) ok = false;
                            else
                                for (auto& o : cand)
                                    if (o.general && o.follow >= 0 && o.contacts[o.follow].IsSame(e)) bb.UpdateEdge(TopoDS::Edge(made.Find(e)), o.follow_pc, nf, 1e-6);
                            followed_any = true;
                            continue;
                        }
                    }
                    TopoDS_Vertex v1, v2;
                    TopExp::Vertices(e, v1, v2, Standard_False);
                    v1 = mv(v1);
                    v2 = mv(v2);
                    if (tf.general) {
                        // the new surface's own end: the conic between its
                        // corners (a kept one stays, its curve put on the new face)
                        if (made.IsBound(e)) continue;
                        bool kp = false;
                        for (size_t i = 0; i < tf.sections.size(); i++) kp |= tf.kept[i] && tf.sections[i].IsSame(e);
                        if (kp) continue;
                        double f0, f1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                        gp_Pnt2d pm = pc->Value(0.5 * (f0 + f1));
                        double at = tf.sec_u ? pm.Y() : pm.X();
                        double s0 = tf.sec_u ? tf.v0 : tf.u0, s1 = tf.sec_u ? tf.v1 : tf.u1;
                        opencascade::handle<Geom_Curve> iso = std::fabs(at - s0) < std::fabs(at - s1) ? surf->VIso(s0) : surf->VIso(s1);
                        gp_Pnt p1 = BRep_Tool::Pnt(v1);
                        bool fwd = iso->Value(iso->FirstParameter()).Distance(p1) < iso->Value(iso->LastParameter()).Distance(p1);
                        if (!fwd) iso = iso->Reversed();
                        BRepBuilderAPI_MakeEdge me(iso, v1, v2);
                        if (!me.IsDone()) {
                            ok = false;
                            break;
                        }
                        made.Bind(e, me.Edge());
                        bound_e.push_back(e);
                        continue;
                    }
                    double f0, f1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                    gp_Pnt2d pm = pc->Value(0.5 * (f0 + f1));
                    bool seam = BRep_Tool::IsClosed(e, tf.f);
                    // where it sits in the sweep, and the conic there
                    double at;
                    std::function<gp_Pnt(const gp_Pnt&)> m;
                    bool flat_line = true;
                    if (tf.torus) {
                        at = pm.X() - tf.u0;
                        if (seam) at = 0.0;
                        gp_Trsf r;
                        r.SetRotation(gp_Ax1(o, z), at);
                        m = [r](const gp_Pnt& p) { return p.Transformed(r); };
                    } else {
                        // the plane it lies on (an end face), or square to the axis
                        gp_Dir n = z;
                        double dist = pm.Y() - tf.v0;
                        gp_Pnt q = tf.gen->StartPoint().Translated(gp_Vec(z) * dist);
                        section_plane(tf, e, n, q);
                        flat_line = n.IsParallel(z, 1e-9);
                        double nz = gp_Vec(n).Dot(gp_Vec(z));
                        gp_Vec vz(z), vn(n);
                        m = [=](const gp_Pnt& p) { return p.Translated(vz * (vn.Dot(gp_Vec(p, q)) / nz)); };
                        at = dist;
                    }
                    TopoDS_Edge ne;
                    // whether its ends follow the conic's way (a section may
                    // be part of it, from an end to where it meets another)
                    opencascade::handle<Geom_BezierCurve> c = ten_map(tf.gen, m, false);
                    auto way = [&](const TopoDS_Vertex& a, const TopoDS_Vertex& b) {
                        GeomAPI_ProjectPointOnCurve ja(BRep_Tool::Pnt(a), c), jb(BRep_Tool::Pnt(b), c);
                        if (ja.NbPoints() == 0 || jb.NbPoints() == 0) return true;
                        return ja.LowerDistanceParameter() < jb.LowerDistanceParameter();
                    };
                    bool same_way = true;
                    if (made.IsBound(e)) {
                        ne = TopoDS::Edge(made.Find(e));
                        same_way = way(TopExp::FirstVertex(ne), TopExp::LastVertex(ne));
                    } else {
                        same_way = way(v1, v2);
                        BRepBuilderAPI_MakeEdge me(same_way ? c : ten_map(tf.gen, m, true), v1, v2);
                        if (!me.IsDone()) {
                            ok = false;
                            break;
                        }
                        ne = me.Edge();
                        made.Bind(e, ne);
                        bound_e.push_back(e);
                    }
                    // its (sweep, t) on the new face
                    auto line = [&](double sweep) -> opencascade::handle<Geom2d_Curve> {
                        if (tf.torus) return same_way ? new Geom2d_Line(gp_Pnt2d(sweep, 0), gp_Dir2d(0, 1)) : new Geom2d_Line(gp_Pnt2d(sweep, 1), gp_Dir2d(0, -1));
                        return same_way ? new Geom2d_Line(gp_Pnt2d(0, sweep), gp_Dir2d(1, 0)) : new Geom2d_Line(gp_Pnt2d(1, sweep), gp_Dir2d(-1, 0));
                    };
                    if (seam) {
                        // the pcurve of its forward use first
                        double g0, g1;
                        opencascade::handle<Geom2d_Curve> pf = BRep_Tool::CurveOnSurface(TopoDS::Edge(e.Oriented(TopAbs_FORWARD)), tf.f, g0, g1);
                        bool first = ten_near_mod(pf->Value(0.5 * (g0 + g1)).X(), tf.u0) && std::fabs(pf->Value(0.5 * (g0 + g1)).X() - tf.u0) < 1e-6;
                        double span = tf.u1 - tf.u0;
                        bb.UpdateEdge(ne, first ? line(0) : line(span), first ? line(span) : line(0), nf, 1e-6);
                    } else if (flat_line) {
                        bb.UpdateEdge(ne, line(at), nf, 1e-6);
                    }
                }
                if (!ok && getenv("FORME_OCCT_DEBUG")) fprintf(stderr, "tension: face of fillet %d left round\n", tf.which);
                if (!ok) {
                    // left as it is: what was remade for it goes too
                    for (auto& x : bound_e) made.UnBind(x);
                    for (auto& x : bound_v) vmap.UnBind(x);
                    continue;
                }
                // a contact followed: its row of the new surface
                if (tf.general && tf.follow >= 0) bb.UpdateEdge(tf.follow_edge, new Geom2d_Line(gp_Pnt2d(tf.follow_u, 0.0), gp_Dir2d(0, 1)), nf, 1e-6);
                // a contact shrunk to a point: the new surface's row that is
                // that point (its place along kept: the rows' parameter is
                // the old face's along it)
                for (const TopoDS_Edge& e : tf.contacts) {
                    if (!tf.general || !BRep_Tool::Degenerated(e)) continue;
                    double f0, f1;
                    opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, tf.f, f0, f1);
                    gp_Pnt2d a = pc->Value(f0), b = pc->Value(f1);
                    double xa = tf.sec_u ? a.Y() : a.X(), xb = tf.sec_u ? b.Y() : b.X();
                    gp_Pnt pt = BRep_Tool::Pnt(TopExp::FirstVertex(e));
                    double mid = 0.5 * (xa + xb);
                    double ur = surf->Value(0.0, mid).Distance(pt) < surf->Value(1.0, mid).Distance(pt) ? 0.0 : 1.0;
                    if (surf->Value(ur, mid).Distance(pt) > 1e-6) {
                        ok = false;
                        break;
                    }
                    TColgp_Array1OfPnt2d poles(1, 2);
                    poles(1) = gp_Pnt2d(ur, xa);
                    poles(2) = gp_Pnt2d(ur, xb);
                    TColStd_Array1OfReal knots(1, 2);
                    knots(1) = f0;
                    knots(2) = f1;
                    TColStd_Array1OfInteger mults(1, 2);
                    mults(1) = mults(2) = 2;
                    bb.UpdateEdge(e, opencascade::handle<Geom2d_Curve>(new Geom2d_BSplineCurve(poles, knots, mults, 1)), nf, BRep_Tool::Tolerance(e));
                }
                if (!ok) {
                    if (getenv("FORME_OCCT_DEBUG")) fprintf(stderr, "tension: face of fillet %d left round (its point contact)\n", tf.which);
                    for (auto& x : bound_e) made.UnBind(x);
                    for (auto& x : bound_v) vmap.UnBind(x);
                    continue;
                }
                rewire(tf.f, nf);
                if (tf.general || followed_any) cover(nf, surf);
                rs->Replace(tf.f, nf);
                if (tf.which >= 0) done[tf.which]++;
            }
            // the corners round another fillet's rounded edge
            for (const TenCorner& c : corners) {
                double w;
                if (!corner_w(c, w)) continue;
                bool along = in_cand(c.cy) != nullptr;
                if (!along && std::fabs(w - w_circle) < 1e-9) continue;  // nothing changes
                // its run: the cylinder's new section there, else its old circle
                TopoDS_Edge be = c.base;
                if (along) {
                    if (!made.IsBound(c.base)) continue;
                    be = TopoDS::Edge(made.Find(c.base));
                }
                BRepAdaptor_Curve bc(be);
                double c0 = bc.FirstParameter(), c1 = bc.LastParameter();
                gp_Lin ax(c.o, c.a);
                gp_Vec va(c.a);
                std::vector<gp_Pnt> vs;
                for (TopExp_Explorer ex(c.f, TopAbs_VERTEX); ex.More(); ex.Next()) vs.push_back(BRep_Tool::Pnt(TopoDS::Vertex(ex.Current())));
                auto snap = [&](const gp_Pnt& p) {
                    for (auto& v : vs)
                        if (v.Distance(p) < 1e-5) return v;
                    return p;
                };
                const int N = 64;
                opencascade::handle<TColgp_HArray1OfPnt> rows[3];
                for (auto& r : rows) r = new TColgp_HArray1OfPnt(1, N + 1);
                opencascade::handle<TColStd_HArray1OfReal> par = new TColStd_HArray1OfReal(1, N + 1);
                double sg = 1.0;
                bool ok = true;
                for (int j = 0; j <= N && ok; j++) {
                    double t = c0 + (c1 - c0) * j / N;
                    gp_Pnt p;
                    gp_Vec d;
                    bc.D1(t, p, d);
                    // the section square to the run: up the axis, then in (or out)
                    gp_Vec nn = va.Crossed(d);
                    if (nn.Magnitude() < 1e-12) {
                        ok = false;
                        break;
                    }
                    nn.Normalize();
                    if (j == 0) {
                        gp_Vec rad(ax.Location(), p);
                        rad -= va * rad.Dot(va);
                        sg = rad.Dot(nn) >= 0 ? 1.0 : -1.0;
                    }
                    gp_Pnt k = p.Translated(va * c.h);
                    gp_Pnt q = k.Translated(nn * (sg * c.sr));
                    if (j == 0 || j == N) {
                        p = snap(p);
                        q = snap(q);
                    }
                    rows[0]->SetValue(j + 1, p);
                    rows[1]->SetValue(j + 1, k);
                    rows[2]->SetValue(j + 1, q);
                    par->SetValue(j + 1, t);
                }
                if (!ok) continue;
                opencascade::handle<Geom_BSplineCurve> cr[3];
                opencascade::handle<Geom_BSplineSurface> g = ten_rows(rows, par, w, cr);
                if (g.IsNull()) continue;
                {
                    BRepAdaptor_Surface os(c.f, Standard_False);
                    double u0, u1, v0, v1;
                    BRepTools::UVBounds(c.f, u0, u1, v0, v1);
                    gp_Pnt op, np;
                    gp_Vec ou, ov, nu, nv;
                    os.D1(0.5 * (u0 + u1), 0.5 * (v0 + v1), op, ou, ov);
                    g->D1(0.5, 0.5 * (c0 + c1), np, nu, nv);
                    if (ou.Crossed(ov).Dot(nu.Crossed(nv)) < 0) g->UReverse();
                }
                if (along) {
                    // its edge on the flat face follows the run too
                    TopoDS_Vertex t1, t2;
                    TopExp::Vertices(c.top, t1, t2, Standard_False);
                    opencascade::handle<Geom_Curve> tc = cr[2];
                    if (cr[2]->StartPoint().Distance(BRep_Tool::Pnt(t1)) > cr[2]->EndPoint().Distance(BRep_Tool::Pnt(t1))) tc = cr[2]->Reversed();
                    BRepBuilderAPI_MakeEdge me(tc, mv(t1), mv(t2));
                    if (!me.IsDone()) continue;
                    TopoDS_Edge te = me.Edge();
                    ShapeFix_Edge sfe;
                    sfe.FixAddPCurve(te, c.pl, Standard_False, 1e-6);
                    made.Bind(c.top, te);
                }
                TopoDS_Face nf;
                bb.MakeFace(nf, g, 1e-7);
                rewire(c.f, nf);
                cover(nf, g);
                rs->Replace(c.f, nf);
                // its own fillet's tension when it follows it
                if (c.which >= 0 && along) done[c.which]++;
            }
            // the sphere corners: a rational patch through the three new
            // ends, tangent to the three fillets (a sphere's eighth again
            // with a circle's tension)
            for (auto& c : spheres) {
                std::vector<TopoDS_Edge> ce;
                int old = 0;
                for (TopExp_Explorer ex(c.first, TopAbs_EDGE); ex.More(); ex.Next()) {
                    if (BRep_Tool::Degenerated(TopoDS::Edge(ex.Current()))) continue;
                    old++;
                    if (made.IsBound(ex.Current())) ce.push_back(TopoDS::Edge(made.Find(ex.Current())));
                }
                if (old != 3) continue;
                if (ce.size() != 3) continue;
                // the cylinder each end belongs to: its axis
                auto axis = [&](const TopoDS_Edge& ne, gp_Dir& z) {
                    for (TopExp_Explorer ex(c.first, TopAbs_EDGE); ex.More(); ex.Next()) {
                        if (!made.IsBound(ex.Current()) || !made.Find(ex.Current()).IsSame(ne)) continue;
                        for (TopTools_ListOfShape::Iterator it(e2f.FindFromKey(ex.Current())); it.More(); it.Next())
                            if (const TenFace* o = in_cand(it.Value())) {
                                z = o->frame.Direction();
                                return true;
                            }
                    }
                    return false;
                };
                // a conic end from vertex a: its middle pole and weight
                auto conic_from = [&](const TopoDS_Edge& e, const TopoDS_Vertex& a, gp_Pnt& mid, double& w, bool& fwd) {
                    double t0, t1;
                    opencascade::handle<Geom_BezierCurve> bz = opencascade::handle<Geom_BezierCurve>::DownCast(BRep_Tool::Curve(e, t0, t1));
                    if (bz.IsNull() || bz->NbPoles() != 3 || std::fabs(t0) > 1e-9 || std::fabs(t1 - 1.0) > 1e-9) return false;
                    fwd = bz->Pole(1).Distance(BRep_Tool::Pnt(a)) < 1e-6;
                    mid = bz->Pole(2);
                    w = bz->Weight(2) / std::sqrt(bz->Weight(1) * bz->Weight(3));
                    return true;
                };
                TopoDS_Vertex P, Q, R, x;
                if (!TopExp::CommonVertex(ce[0], ce[1], P)) continue;
                TopoDS_Vertex a0, a1, b0, b1;
                TopExp::Vertices(ce[0], a0, a1);
                TopExp::Vertices(ce[1], b0, b1);
                Q = a0.IsSame(P) ? a1 : a0;
                R = b0.IsSame(P) ? b1 : b0;
                TopExp::Vertices(ce[2], a0, a1);
                if (!((a0.IsSame(Q) && a1.IsSame(R)) || (a0.IsSame(R) && a1.IsSame(Q)))) continue;
                gp_Pnt mb, mc, ma;
                double wb, wc, wa;
                bool fb, fc, fa;
                if (!conic_from(ce[0], P, mb, wb, fb) || !conic_from(ce[1], P, mc, wc, fc) || !conic_from(ce[2], Q, ma, wa, fa)) continue;
                if (std::fabs(wb - wc) > 1e-9 || std::fabs(wb - wa) > 1e-9) continue;
                gp_Pnt p = BRep_Tool::Pnt(P), q = BRep_Tool::Pnt(Q), r = BRep_Tool::Pnt(R);
                // the inner pole, from either side; along each fillet's axis
                // out of the corner's sides (so it meets them tangent)
                gp_Pnt k = mb.Translated(gp_Vec(q, ma)), k2 = mc.Translated(gp_Vec(r, ma));
                double size = p.Distance(q) + p.Distance(r);
                gp_Dir zb, zc, za;
                if (k.Distance(k2) > 1e-6 * size || !axis(ce[0], zb) || !axis(ce[1], zc) || !axis(ce[2], za)) continue;
                auto along = [&](const gp_Pnt& a, const gp_Pnt& b, const gp_Dir& z) {
                    gp_Vec v(a, b);
                    return v.Magnitude() > 1e-9 && v.Normalized().Crossed(gp_Vec(z)).Magnitude() < 1e-6;
                };
                if (!along(q, ma, zb) || !along(r, ma, zc) || !along(mb, q, za)) continue;
                TColgp_Array2OfPnt poles(1, 3, 1, 3);
                TColStd_Array2OfReal wts(1, 3, 1, 3);
                gp_Pnt rows[3][3] = {{p, p, p}, {mb, k, mc}, {q, ma, r}};
                double wi[3] = {1.0, wb, 1.0};
                for (int i = 0; i < 3; i++)
                    for (int j = 0; j < 3; j++) {
                        poles.SetValue(i + 1, j + 1, rows[i][j]);
                        wts.SetValue(i + 1, j + 1, wi[i] * wi[j]);
                    }
                opencascade::handle<Geom_BezierSurface> bs = new Geom_BezierSurface(poles, wts);
                TopoDS_Face nf;
                bb.MakeFace(nf, bs, 1e-7);
                // its outline counter-clockwise in (u, v): P → Q along v = 0,
                // Q → R along u = 1, R → P along v = 1, the pole back along u = 0
                auto lin = [](double x0, double y0, double dx, double dy) -> opencascade::handle<Geom2d_Curve> { return new Geom2d_Line(gp_Pnt2d(x0, y0), gp_Dir2d(dx, dy)); };
                bb.UpdateEdge(ce[0], fb ? lin(0, 0, 1, 0) : lin(1, 0, -1, 0), nf, 1e-6);
                bb.UpdateEdge(ce[2], fa ? lin(1, 0, 0, 1) : lin(1, 1, 0, -1), nf, 1e-6);
                bb.UpdateEdge(ce[1], fc ? lin(0, 1, 1, 0) : lin(1, 1, -1, 0), nf, 1e-6);
                TopoDS_Edge de;
                bb.MakeEdge(de);
                bb.UpdateEdge(de, lin(0, 0, 0, 1), nf, 1e-6);
                bb.Range(de, 0.0, 1.0);
                bb.Add(de, P.Oriented(TopAbs_FORWARD));
                bb.Add(de, P.Oriented(TopAbs_REVERSED));
                bb.Degenerated(de, Standard_True);
                TopoDS_Wire nw;
                bb.MakeWire(nw);
                bb.Add(nw, ce[0].Oriented(fb ? TopAbs_FORWARD : TopAbs_REVERSED));
                bb.Add(nw, ce[2].Oriented(fa ? TopAbs_FORWARD : TopAbs_REVERSED));
                bb.Add(nw, ce[1].Oriented(fc ? TopAbs_REVERSED : TopAbs_FORWARD));
                bb.Add(nw, de.Oriented(TopAbs_REVERSED));
                bb.Add(nf, nw);
                // facing out as the sphere did
                BRepAdaptor_Surface os(c.first, Standard_True);
                gp_Pnt op, np;
                gp_Vec ou, ov, nu, nv2;
                os.D1(0.5 * (os.FirstUParameter() + os.LastUParameter()), 0.5 * (os.FirstVParameter() + os.LastVParameter()), op, ou, ov);
                gp_Vec on = ou.Crossed(ov);
                if (c.first.Orientation() == TopAbs_REVERSED) on.Reverse();
                bs->D1(0.5, 0.5, np, nu, nv2);
                if (on.Dot(nu.Crossed(nv2)) < 0) nf.Orientation(TopAbs_REVERSED);
                BRepLib::SameParameter(nf, 1e-6);
                rs->Replace(c.first, nf);
                done[c.second]++;
            }
            int any = 0;
            for (int k = 0; k < n; k++) any += done[k];
            bool told = false;
            for (int k = 0; k < n; k++) told |= tight[4 * k + 1] > 0.0;
            if (any == 0 && told) {
                // nothing made conic, for a reason `tight` tells: the shape as it was
                FShape* res = wrap(s->s);
                res->tags = s->tags;
                return res;
            }
            if (any == 0) {
                set_err("tension: only on fillets along straight or circular edges, ending on flat faces or a fillet of the same size and tension");
                return nullptr;
            }
            // (both forward: the new edge runs between the same vertices the same way)
            for (TopTools_DataMapOfShapeShape::Iterator it(made); it.More(); it.Next()) rs->Replace(it.Key().Oriented(TopAbs_FORWARD), it.Value().Oriented(TopAbs_FORWARD));
            TopoDS_Shape r = rs->Apply(s->s);
            ShapeFix_Shape fix(r);
            fix.SetContext(rs);
            fix.Perform();
            r = fix.Shape();
            if (!valid(r)) {
                dump_check(r);
                if (getenv("FORME_OCCT_DEBUG"))
                    for (TopExp_Explorer ex(r, TopAbs_FACE); ex.More(); ex.Next()) {
                        BRepCheck_Analyzer fa(ex.Current());
                        if (fa.IsValid()) continue;
                        BRepAdaptor_Surface fs(TopoDS::Face(ex.Current()), Standard_False);
                        int ne = 0;
                        for (TopExp_Explorer ee(ex.Current(), TopAbs_EDGE); ee.More(); ee.Next()) ne++;
                        fprintf(stderr, "invalid face: surface type %d, %d edges, orientation %d\n", (int)fs.GetType(), ne, (int)ex.Current().Orientation());
                        dump_check(ex.Current());
                        for (TopExp_Explorer ee(ex.Current(), TopAbs_EDGE); ee.More(); ee.Next()) {
                            BRepAdaptor_Curve ec(TopoDS::Edge(ee.Current()));
                            gp_Pnt a = ec.Value(ec.FirstParameter()), b = ec.Value(ec.LastParameter());
                            double g0, g1;
                            opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(TopoDS::Edge(ee.Current()), TopoDS::Face(ex.Current()), g0, g1);
                            gp_Pnt2d p0 = pc.IsNull() ? gp_Pnt2d() : pc->Value(g0), p1 = pc.IsNull() ? gp_Pnt2d() : pc->Value(g1);
                            fprintf(stderr, "  edge %d %s (%.2f %.2f %.2f) -> (%.2f %.2f %.2f), pc (%.2f %.2f) -> (%.2f %.2f) range %.3f %.3f / %.3f %.3f\n", (int)ec.GetType(), ee.Current().Orientation() == TopAbs_FORWARD ? "F" : "R", a.X(), a.Y(), a.Z(), b.X(), b.Y(), b.Z(), p0.X(), p0.Y(), p1.X(), p1.Y(), ec.FirstParameter(), ec.LastParameter(), g0, g1);
                            if (!pc.IsNull()) {
                                opencascade::handle<Geom_Surface> gs = BRep_Tool::Surface(TopoDS::Face(ex.Current()));
                                double d = 0;
                                for (int k = 0; k <= 32; k++) {
                                    double t = ec.FirstParameter() + (ec.LastParameter() - ec.FirstParameter()) * k / 32.0;
                                    gp_Pnt2d uv = pc->Value(g0 + (g1 - g0) * k / 32.0);
                                    d = std::max(d, gs->Value(uv.X(), uv.Y()).Distance(ec.Value(t)));
                                }
                                fprintf(stderr, "    tol %.3g off %.3g sameparam %d samerange %d\n", BRep_Tool::Tolerance(TopoDS::Edge(ee.Current())), d, (int)BRep_Tool::SameParameter(TopoDS::Edge(ee.Current())), (int)BRep_Tool::SameRange(TopoDS::Edge(ee.Current())));
                            }
                        }
                    }
                set_err("tension gave an invalid solid");
                return nullptr;
            }
            FShape* res = wrap(r);
            // the tags: each face of the input to what it became
            TopTools_IndexedMapOfShape om;
            face_map(r, om);
            res->tags.assign(om.Extent(), tags[0]);
            for (int i = 1; i <= fm.Extent(); i++) {
                long long t = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                TopoDS_Shape g = rs->Value(fm(i));
                for (TopExp_Explorer ex(g, TopAbs_FACE); ex.More(); ex.Next()) {
                    int j = om.FindIndex(ex.Current());
                    if (j > 0) res->tags[j - 1] = t;
                }
            }
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
            const FShape copy = pristine(s);
            const FShape* s = &copy;
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
            const FShape copy = pristine(s);
            const FShape* s = &copy;
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
            // facing like the face it comes from
            gp_Dir want = pl.Axis().Direction();
            if (f.Orientation() == TopAbs_REVERSED) want.Reverse();
            auto facing = [&](TopoDS_Face r) {
                BRepAdaptor_Surface rs(r);
                gp_Dir got = rs.Plane().Axis().Direction();
                if (r.Orientation() == TopAbs_REVERSED) got.Reverse();
                if (got.Dot(want) < 0) r.Reverse();
                return r;
            };
            if (d > 0) {
                // Each loop on its own (Romain, 19 nov.: insets that failed
                // where a hole comes near the border): the outline moved in
                // (it may part into several pieces), each hole grown, then
                // the outline's inside less the grown holes, a 2D boolean on
                // the plane. One face, or several when it parts.
                auto offset = [&](const TopoDS_Wire& w, double off, std::vector<TopoDS_Face>& out) -> bool {
                    TopoDS_Wire cw = rebuild(w);
                    if (cw.IsNull()) return false;
                    TopoDS_Face base = disc(cw);
                    BRepOffsetAPI_MakeOffset mo(base, GeomAbs_Intersection);
                    mo.Perform(off);
                    if (!mo.IsDone() || mo.Shape().IsNull()) return true; // nothing left
                    TopoDS_Shape ws = mo.Shape();
                    BRepLib::BuildCurves3d(ws);
                    for (TopExp_Explorer x(ws, TopAbs_WIRE); x.More(); x.Next()) {
                        TopoDS_Wire nw = rebuild(TopoDS::Wire(x.Current()));
                        if (nw.IsNull()) return false;
                        TopoDS_Face nf = disc(nw);
                        if (area(nf) > 1e-6) out.push_back(nf);
                    }
                    return true;
                };
                TopoDS_Wire outer_w = BRepTools::OuterWire(fw);
                std::vector<TopoDS_Face> inside, holes;
                if (!offset(outer_w, -d, inside)) {
                    set_err("the inset outline is not closed");
                    return nullptr;
                }
                if (inside.empty()) {
                    set_err("the inset is wider than the face");
                    return nullptr;
                }
                for (TopExp_Explorer x(fw, TopAbs_WIRE); x.More(); x.Next()) {
                    TopoDS_Wire w = TopoDS::Wire(x.Current());
                    if (w.IsSame(outer_w)) continue;
                    std::vector<TopoDS_Face> grown;
                    if (!offset(w, d, grown) || grown.empty()) {
                        set_err("a hole of the inset could not be made");
                        return nullptr;
                    }
                    // the grown hole: its biggest piece
                    size_t big = 0;
                    for (size_t i = 1; i < grown.size(); i++)
                        if (area(grown[i]) > area(grown[big])) big = i;
                    holes.push_back(grown[big]);
                }
                std::vector<TopoDS_Face> parts;
                if (holes.empty()) {
                    parts = inside;
                } else {
                    TopTools_ListOfShape args, tools;
                    for (auto& x : inside) args.Append(x);
                    for (auto& x : holes) tools.Append(x);
                    BRepAlgoAPI_Cut cut;
                    cut.SetArguments(args);
                    cut.SetTools(tools);
                    cut.Build();
                    if (!cut.IsDone()) {
                        set_err("the inset's holes could not be made");
                        return nullptr;
                    }
                    for (TopExp_Explorer x(cut.Shape(), TopAbs_FACE); x.More(); x.Next()) {
                        TopoDS_Face pf = TopoDS::Face(x.Current());
                        if (std::fabs(area(pf)) > 1e-6) parts.push_back(pf);
                    }
                }
                if (parts.empty()) {
                    set_err("the inset is wider than the face");
                    return nullptr;
                }
                if (parts.size() == 1) return wrap(facing(parts[0]));
                TopoDS_Compound c;
                BRep_Builder cb;
                cb.MakeCompound(c);
                for (auto& x : parts) cb.Add(c, facing(x));
                return wrap(c);
            }
            // d = 0: the face's own wires
            TopoDS_Shape ws = fw;
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
            return wrap(facing(r));
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
                // an offset curve's edges may sit a little off their faces
                // (an inset of a curved outline): mended, then checked again
                ShapeFix_Shape fix(r);
                fix.SetPrecision(1e-6);
                fix.SetMaxTolerance(1e-3);
                fix.Perform();
                if (valid(fix.Shape())) r = fix.Shape();
            }
            if (!valid(r)) {
                dump_check(r);
                set_err("extrusion gave an invalid solid");
                return nullptr;
            }
            return wrap(r);
        },
        (FShape*)nullptr);
}

// Face `index` (0-based) of a solid copied and made a solid `t` thick
// along its outward normal (< 0: inwards): the tool an extrusion of a
// curved face fuses (or cuts). Tags: the face it starts from `base`, its
// offset `top`, the sides `side`, `side` + 1…
FShape* focc_thicken_face(const FShape* s, int index, double t, long long base, long long top, long long side) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            if (index < 0 || index >= fm.Extent()) {
                set_err("no such face");
                return nullptr;
            }
            if (std::fabs(t) < 1e-6) {
                set_err("no depth");
                return nullptr;
            }
            TopoDS_Shape c = BRepBuilderAPI_Copy(fm(index + 1)).Shape();
            BRepOffset_MakeOffset mo;
            mo.Initialize(c, t, 1e-5, BRepOffset_Skin, Standard_False, Standard_False, GeomAbs_Intersection, Standard_True, Standard_False);
            mo.MakeOffsetShape();
            if (!mo.IsDone() || mo.Shape().IsNull()) {
                set_err(t < 0 ? "the face cannot be pushed in that far (its curve is tighter)" : "the face could not be pulled out");
                return nullptr;
            }
            TopoDS_Shape r = mo.Shape();
            if (!valid(r)) {
                ShapeFix_Shape fix(r);
                fix.SetPrecision(1e-6);
                fix.SetMaxTolerance(1e-3);
                fix.Perform();
                if (valid(fix.Shape())) r = fix.Shape();
            }
            if (!valid(r)) {
                dump_check(r);
                set_err("the extrusion gave an invalid solid");
                return nullptr;
            }
            FShape* res = wrap(r);
            if (!res) return res;
            TopTools_IndexedMapOfShape fout;
            face_map(res->s, fout);
            res->tags.assign(fout.Extent(), base);
            auto put = [&](const TopTools_ListOfShape& g, long long tg) {
                for (TopTools_ListOfShape::Iterator it(g); it.More(); it.Next())
                    for (TopExp_Explorer ex(it.Value(), TopAbs_FACE); ex.More(); ex.Next()) {
                        int j = fout.FindIndex(ex.Current());
                        if (j > 0) res->tags[j - 1] = tg;
                    }
            };
            for (TopExp_Explorer ex(c, TopAbs_FACE); ex.More(); ex.Next()) put(mo.Generated(ex.Current()), top);
            TopTools_IndexedMapOfShape ce;
            edge_map(c, ce);
            for (int k = 1; k <= ce.Extent(); k++) put(mo.Generated(ce(k)), side + k - 1);
            return res;
        },
        (FShape*)nullptr);
}

static FShape* unify_across(FShape* res, const TopTools_MapOfShape& across, const TopTools_MapOfShape* ends);

// Flat face `index` (0-based) of a solid moved `d` along its outward
// normal, the faces round it (flat or curved, they must run along the
// move: a box's sides, a hole's wall, a boss's side) made longer or
// shorter: the prism the face sweeps fused (d > 0) or cut (d < 0), its
// sides then made one with the faces they extend. Tags kept: the moved
// face its own, each side its neighbour's.
FShape* focc_move_face(const FShape* s, int index, double d) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm0, fm;
            face_map(s->s, fm0);
            if (index < 0 || index >= fm0.Extent()) {
                set_err("no such face");
                return nullptr;
            }
            if (std::fabs(d) < 1e-6) {
                set_err("no move");
                return nullptr;
            }
            // the solid with its cylinders in one frame each (the sides
            // the prism adds must share the frame of the faces they extend)
            std::unique_ptr<FShape> bs(wrap(canon_cylinders(s->s)));
            if (!bs) return nullptr;
            face_map(bs->s, fm);
            if (fm.Extent() != fm0.Extent()) {
                set_err("the faces could not be read");
                return nullptr;
            }
            bs->tags = s->tags;
            TopoDS_Face F = TopoDS::Face(fm(index + 1));
            BRepAdaptor_Surface sa(F, Standard_False);
            if (sa.GetType() != GeomAbs_Plane) {
                set_err("only a flat face moves along its normal this way");
                return nullptr;
            }
            gp_Dir n = sa.Plane().Axis().Direction();
            if (F.Orientation() == TopAbs_REVERSED) n.Reverse();
            auto tag_of = [&](const TopoDS_Shape& f) -> long long {
                int i = fm.FindIndex(f);
                return i > 0 && i - 1 < (int)bs->tags.size() ? bs->tags[i - 1] : -1;
            };
            // the faces round it run along the move
            TopTools_IndexedDataMapOfShapeListOfShape ef;
            TopExp::MapShapesAndAncestors(bs->s, TopAbs_EDGE, TopAbs_FACE, ef);
            TopTools_IndexedMapOfShape fe;
            edge_map(F, fe);
            std::vector<long long> side(fe.Extent() + 1, tag_of(F));
            for (int k = 1; k <= fe.Extent(); k++) {
                TopoDS_Edge e = TopoDS::Edge(fe(k));
                if (BRep_Tool::Degenerated(e) || !ef.Contains(e)) continue;
                BRepAdaptor_Curve c(e);
                gp_Pnt p = c.Value(0.5 * (c.FirstParameter() + c.LastParameter()));
                for (TopTools_ListIteratorOfListOfShape it(ef.FindFromKey(e)); it.More(); it.Next()) {
                    if (it.Value().IsSame(F)) continue;
                    TopoDS_Face G = TopoDS::Face(it.Value());
                    opencascade::handle<Geom_Surface> S = BRep_Tool::Surface(G);
                    if (S.IsNull()) continue;
                    ShapeAnalysis_Surface sas(S);
                    gp_Pnt2d uv = sas.ValueOfUV(p, 1e-7);
                    GeomLProp_SLProps pr(S, uv.X(), uv.Y(), 1, 1e-9);
                    if (pr.IsNormalDefined() && std::fabs(pr.Normal().Dot(n)) > 1e-3) {
                        set_err("a face round it does not run along the move (a fillet, a slanted face): it cannot follow yet");
                        return nullptr;
                    }
                    side[k] = tag_of(G);
                }
            }
            gp_Vec v(n.XYZ() * d);
            BRepPrimAPI_MakePrism mp(F, v, Standard_False, Standard_True);
            if (!mp.IsDone()) {
                set_err("the face could not sweep its way");
                return nullptr;
            }
            TopTools_IndexedMapOfShape pf;
            face_map(mp.Shape(), pf);
            std::vector<long long> ptags(pf.Extent(), tag_of(F));
            std::vector<int> sides;
            for (int k = 1; k <= fe.Extent(); k++)
                for (TopTools_ListIteratorOfListOfShape it(mp.Generated(fe(k))); it.More(); it.Next()) {
                    int j = pf.FindIndex(it.Value());
                    if (j > 0) {
                        ptags[j - 1] = side[k];
                        sides.push_back(j);
                    }
                }
            std::unique_ptr<FShape> prism(wrap(canon_cylinders(mp.Shape())));
            if (!prism) return nullptr;
            TopTools_IndexedMapOfShape pf2;
            face_map(prism->s, pf2);
            if (pf2.Extent() != pf.Extent()) {
                set_err("the face could not sweep its way");
                return nullptr;
            }
            prism->tags = ptags;
            TopoDS_Shape r;
            FShape* res = nullptr;
            auto finish = [&](BRepAlgoAPI_BooleanOperation& op) -> bool {
                op.SetFuzzyValue(1e-6);
                op.Build();
                if (!op.IsDone() || op.HasErrors()) return false;
                r = op.Shape();
                int solids = 0;
                TopoDS_Shape one;
                for (TopExp_Explorer x(r, TopAbs_SOLID); x.More(); x.Next()) {
                    solids++;
                    one = x.Current();
                }
                if (solids != 1) return false;
                res = wrap(one);
                if (!res) return false;
                carry_tags(op, bs.get(), res, -1);
                carry_tags(op, prism.get(), res, -1);
                // the sides made one with the faces they extend: across the
                // face's old edges, and any edge between a side and a face
                // of its tag (a notch's edge it now meets)
                TopTools_MapOfShape across, ends, sidef;
                for (int k = 1; k <= fe.Extent(); k++) {
                    const TopTools_ListOfShape& im = op.Modified(fe(k));
                    if (im.IsEmpty() && !op.IsDeleted(fe(k))) across.Add(fe(k));
                    for (TopTools_ListIteratorOfListOfShape it(im); it.More(); it.Next()) across.Add(it.Value());
                }
                for (int j : sides) {
                    const TopoDS_Shape& f = pf2(j);
                    const TopTools_ListOfShape& im = op.Modified(f);
                    if (im.IsEmpty() && !op.IsDeleted(f)) sidef.Add(f);
                    for (TopTools_ListIteratorOfListOfShape it(im); it.More(); it.Next()) sidef.Add(it.Value());
                }
                {
                    TopTools_IndexedMapOfShape rf;
                    face_map(res->s, rf);
                    TopTools_IndexedDataMapOfShapeListOfShape ref;
                    TopExp::MapShapesAndAncestors(res->s, TopAbs_EDGE, TopAbs_FACE, ref);
                    for (int i = 1; i <= ref.Extent(); i++) {
                        const TopTools_ListOfShape& fs = ref(i);
                        if (fs.Extent() != 2 || fs.First().IsSame(fs.Last())) continue;
                        if (!sidef.Contains(fs.First()) && !sidef.Contains(fs.Last())) continue;
                        int a = rf.FindIndex(fs.First()), b = rf.FindIndex(fs.Last());
                        if (a > 0 && b > 0 && res->tags[a - 1] == res->tags[b - 1]) across.Add(ref.FindKey(i));
                    }
                }
                for (TopExp_Explorer x(F, TopAbs_VERTEX); x.More(); x.Next()) {
                    const TopTools_ListOfShape& im = op.Modified(x.Current());
                    if (im.IsEmpty()) ends.Add(x.Current());
                    for (TopTools_ListIteratorOfListOfShape it(im); it.More(); it.Next()) ends.Add(it.Value());
                }
                res = unify_across(res, across, &ends);
                return true;
            };
            bool ok;
            if (d > 0) {
                BRepAlgoAPI_Fuse op;
                TopTools_ListOfShape a, t;
                a.Append(bs->s);
                t.Append(prism->s);
                op.SetArguments(a);
                op.SetTools(t);
                ok = finish(op);
            } else {
                BRepAlgoAPI_Cut op;
                TopTools_ListOfShape a, t;
                a.Append(bs->s);
                t.Append(prism->s);
                op.SetArguments(a);
                op.SetTools(t);
                ok = finish(op);
            }
            if (!ok || !res) {
                delete res;
                set_err(d < 0 ? "the face cannot go in that far (the part would come apart)" : "the face could not be moved out");
                return nullptr;
            }
            if (!valid(res->s)) {
                dump_check(res->s);
                delete res;
                set_err("the move gives an invalid solid");
                return nullptr;
            }
            return res;
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

// Edge `edge` (1-based) at fraction `f` of its parameter range: its point
// (3 doubles). Returns 0, or -1.
int focc_edge_point(const FShape* s, int edge, double f, double* xyz) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            edge_map(s->s, m);
            if (edge < 1 || edge > m.Extent()) {
                set_err("no such edge");
                return -1;
            }
            BRepAdaptor_Curve c(TopoDS::Edge(m(edge)));
            gp_Pnt p = c.Value(c.FirstParameter() + f * (c.LastParameter() - c.FirstParameter()));
            xyz[0] = p.X();
            xyz[1] = p.Y();
            xyz[2] = p.Z();
            return 0;
        },
        -1);
}

// Faces of `res` lying on one surface and meeting along an edge of
// `across` made one face (ShapeUpgrade_UnifySameDomain on those edges
// only), with the tag they share; with `ends`, edges meeting at one of
// those vertices on one line or circle made one edge too. Returns `res`
// itself when the result is not valid; `res` is freed otherwise.
static FShape* unify_across(FShape* res, const TopTools_MapOfShape& across, const TopTools_MapOfShape* ends) {
    // every edge kept but those, and a closed face's seam (one face on
    // both its sides: it parts nothing, and a seam kept stops the faces
    // of a closed surface being joined)
    TopTools_MapOfShape keep;
    for (TopExp_Explorer fx(res->s, TopAbs_FACE); fx.More(); fx.Next())
        for (TopExp_Explorer ex(fx.Current(), TopAbs_EDGE); ex.More(); ex.Next())
            if (!across.Contains(ex.Current()) && !BRep_Tool::IsClosed(TopoDS::Edge(ex.Current()), TopoDS::Face(fx.Current()))) keep.Add(ex.Current());
    if (ends)
        for (TopExp_Explorer ex(res->s, TopAbs_VERTEX); ex.More(); ex.Next())
            if (!ends->Contains(ex.Current())) keep.Add(ex.Current());
    ShapeUpgrade_UnifySameDomain u(res->s, ends ? Standard_True : Standard_False, Standard_True, Standard_False);
    u.KeepShapes(keep);
    u.Build();
    TopoDS_Shape out = u.Shape();
    // valid when what it was given was (a part already off, kept going)
    if (out.IsNull() || (!BRepCheck_Analyzer(out).IsValid() && BRepCheck_Analyzer(res->s).IsValid())) return res;
    FShape* r2 = wrap(out);
    if (!r2) return res;
    TopTools_IndexedMapOfShape fin, fout;
    face_map(res->s, fin);
    face_map(out, fout);
    r2->tags.assign(fout.Extent(), -1);
    opencascade::handle<BRepTools_History> h = u.History();
    for (int i = 1; i <= fin.Extent(); i++) {
        long long t = i - 1 < (int)res->tags.size() ? res->tags[i - 1] : -1;
        const TopoDS_Shape& f = fin(i);
        bool found = false;
        if (!h.IsNull())
            for (TopTools_ListIteratorOfListOfShape it(h->Modified(f)); it.More(); it.Next()) {
                int j = fout.FindIndex(it.Value());
                if (j > 0) {
                    r2->tags[j - 1] = t;
                    found = true;
                }
            }
        if (!found) {
            int j = fout.FindIndex(f);
            if (j > 0) r2->tags[j - 1] = t;
        }
    }
    delete res;
    return r2;
}

// After the Knife: a closed face's seam (a cylinder's) that now runs
// between two pieces of that face (a cut across it left the seam inside one
// region, and the splitter made two faces of it) is made a seam again, the
// pieces one face with the tag they share. Returns `res` itself when there
// is nothing to mend or the mending fails.
static FShape* knit_seams(BRepBuilderAPI_MakeShape& op, const FShape* s, FShape* res) {
    TopTools_MapOfShape seams;
    for (TopExp_Explorer fx(s->s, TopAbs_FACE); fx.More(); fx.Next()) {
        TopoDS_Face f = TopoDS::Face(fx.Current());
        for (TopExp_Explorer ex(f, TopAbs_EDGE); ex.More(); ex.Next()) {
            TopoDS_Edge e = TopoDS::Edge(ex.Current());
            if (!BRep_Tool::IsClosed(e, f)) continue;
            const TopTools_ListOfShape& im = op.Modified(e);
            if (im.IsEmpty()) seams.Add(e);
            for (TopTools_ListIteratorOfListOfShape it(im); it.More(); it.Next()) seams.Add(it.Value());
        }
    }
    if (seams.IsEmpty()) return res;
    TopTools_IndexedDataMapOfShapeListOfShape ef;
    TopExp::MapShapesAndAncestors(res->s, TopAbs_EDGE, TopAbs_FACE, ef);
    TopTools_MapOfShape keep;
    bool any = false;
    for (int i = 1; i <= ef.Extent(); i++) {
        const TopoDS_Shape& e = ef.FindKey(i);
        bool apart = false;
        if (seams.Contains(e)) {
            const TopTools_ListOfShape& fs = ef(i);
            for (TopTools_ListIteratorOfListOfShape a(fs); a.More(); a.Next())
                if (!a.Value().IsSame(fs.First())) apart = true;
        }
        if (apart) any = true;
        else keep.Add(e);
    }
    if (!any) return res;
    TopTools_MapOfShape across;
    for (int i = 1; i <= ef.Extent(); i++)
        if (!keep.Contains(ef.FindKey(i))) across.Add(ef.FindKey(i));
    return unify_across(res, across, nullptr);
}

// The Knife: points (n, 3 doubles each, on edges) cut into the edges
// they lie on, and straight lines between pairs of them (segs, 2 ints
// each, lying on faces) cut into those faces. Faces and edges keep their
// tags (both halves of a face cut in two keep its tag).
FShape* focc_split(const FShape* s, const double* pts, int n, const int* segs, int nseg) {
    return guard(
        [&]() -> FShape* {
            BRepAlgoAPI_Splitter op;
            TopTools_ListOfShape args, tools;
            args.Append(s->s);
            std::vector<gp_Pnt> p;
            for (int i = 0; i < n; i++) {
                p.emplace_back(pts[3 * i], pts[3 * i + 1], pts[3 * i + 2]);
                tools.Append(BRepBuilderAPI_MakeVertex(p.back()).Vertex());
            }
            // the face both points lie on: a flat one takes a straight line;
            // across a curved one the line follows the face's own grid
            // (straight in its (u, v)), so it lies on it
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            auto across = [&](const gp_Pnt& a, const gp_Pnt& b, TopoDS_Edge& out) -> bool {
                bool curved = false;
                for (int i = 1; i <= fm.Extent(); i++) {
                    TopoDS_Face f = TopoDS::Face(fm(i));
                    opencascade::handle<Geom_Surface> S = BRep_Tool::Surface(f);
                    if (S.IsNull()) continue;
                    ShapeAnalysis_Surface sas(S);
                    gp_Pnt2d ua = sas.ValueOfUV(a, 1e-7);
                    double ga = sas.Gap();
                    gp_Pnt2d ub = sas.ValueOfUV(b, 1e-7);
                    double gb = sas.Gap();
                    if (ga > 1e-4 || gb > 1e-4) continue;
                    double u0, u1, v0, v1;
                    BRepTools::UVBounds(f, u0, u1, v0, v1);
                    auto into = [](double x, double lo, double hi, bool per, double T) {
                        if (!per) return x;
                        while (x < lo - 1e-7) x += T;
                        while (x > hi + 1e-7) x -= T;
                        return x;
                    };
                    for (gp_Pnt2d* q : {&ua, &ub}) {
                        q->SetX(into(q->X(), u0, u1, S->IsUPeriodic(), S->IsUPeriodic() ? S->UPeriod() : 0));
                        q->SetY(into(q->Y(), v0, v1, S->IsVPeriodic(), S->IsVPeriodic() ? S->VPeriod() : 0));
                    }
                    BRepClass_FaceClassifier ca(f, ua, 1e-4), cb(f, ub, 1e-4);
                    if (ca.State() == TopAbs_OUT || cb.State() == TopAbs_OUT) continue;
                    if (BRepAdaptor_Surface(f, Standard_False).GetType() == GeomAbs_Plane) return false;
                    if (curved) continue;
                    // round a closed face the short way (over its seam)
                    if (S->IsUPeriodic()) {
                        double T = S->UPeriod();
                        if (ub.X() - ua.X() > T / 2) ub.SetX(ub.X() - T);
                        else if (ua.X() - ub.X() > T / 2) ub.SetX(ub.X() + T);
                    }
                    if (S->IsVPeriodic()) {
                        double T = S->VPeriod();
                        if (ub.Y() - ua.Y() > T / 2) ub.SetY(ub.Y() - T);
                        else if (ua.Y() - ub.Y() > T / 2) ub.SetY(ub.Y() + T);
                    }
                    gp_Vec2d d(ua, ub);
                    if (d.Magnitude() < 1e-9) continue;
                    BRepBuilderAPI_MakeEdge me(new Geom2d_Line(ua, gp_Dir2d(d)), S, 0.0, d.Magnitude());
                    if (!me.IsDone()) continue;
                    out = me.Edge();
                    BRepLib::BuildCurves3d(out);
                    curved = true;
                }
                return curved;
            };
            for (int k = 0; k < nseg; k++) {
                int a = segs[2 * k], b = segs[2 * k + 1];
                if (a < 0 || b < 0 || a >= n || b >= n || p[a].Distance(p[b]) < 1e-6) continue;
                TopoDS_Edge on;
                if (across(p[a], p[b], on)) {
                    tools.Append(on);
                    continue;
                }
                BRepBuilderAPI_MakeEdge e(p[a], p[b]);
                if (e.IsDone()) tools.Append(e.Edge());
            }
            if (tools.IsEmpty()) {
                set_err("nothing to cut");
                return nullptr;
            }
            op.SetArguments(args);
            op.SetTools(tools);
            op.SetFuzzyValue(1e-5);
            op.Build();
            if (!op.IsDone() || op.HasErrors()) {
                set_err("the cut failed");
                return nullptr;
            }
            // the part alone: what the splitter made of the argument
            TopoDS_Shape r = op.Shape();
            if (r.ShapeType() != TopAbs_COMPOUND) {
                FShape* res = wrap(r);
                if (!res) return res;
                carry_tags(op, s, res, -1);
                return knit_seams(op, s, res);
            }
            BRep_Builder bb;
            TopoDS_Compound c;
            bb.MakeCompound(c);
            int count = 0;
            TopoDS_Shape one;
            for (TopoDS_Iterator it(r); it.More(); it.Next()) {
                // a tool left on its own (a line off every face, a point
                // off every edge) is dropped
                TopAbs_ShapeEnum t = it.Value().ShapeType();
                if (t == TopAbs_VERTEX) continue;
                if (t == TopAbs_EDGE) {
                    bool tool = false;
                    for (TopTools_ListIteratorOfListOfShape tl(tools); tl.More(); tl.Next())
                        for (TopTools_ListIteratorOfListOfShape im(op.Modified(tl.Value())); im.More(); im.Next())
                            if (im.Value().IsSame(it.Value())) tool = true;
                    for (TopTools_ListIteratorOfListOfShape tl(tools); tl.More(); tl.Next())
                        if (tl.Value().IsSame(it.Value())) tool = true;
                    if (tool) continue;
                }
                bb.Add(c, it.Value());
                count++;
                one = it.Value();
            }
            if (count == 0) {
                set_err("the cut left nothing");
                return nullptr;
            }
            FShape* res = wrap(count == 1 ? one : TopoDS_Shape(c));
            if (!res) return nullptr;
            carry_tags(op, s, res, -1);
            return knit_seams(op, s, res);
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
            // a face with holes or bosses that tilts (stays flat, leaves its
            // plane; 19 nov.): each hole's or boss's wall runs straight along
            // a direction d (a cylinder's axis, the line its flat walls
            // share); its points on the face slide along d onto the new
            // plane, its edges there are the old ones carried the same way,
            // the walls keep their surfaces and only grow or shrink along d
            struct Slide {
                gp_Pnt c;
                gp_Dir n;
                gp_Vec d;
            };
            std::vector<Slide> slides;
            TopTools_DataMapOfShapeInteger slide_of; // a slid vertex → its slide
            TopTools_MapOfShape walls, tilted, slid_e;
            auto slide_s = [&](const Slide& sl, const gp_Pnt& p) { return gp_Vec(p, sl.c).Dot(gp_Vec(sl.n)) / sl.d.Dot(gp_Vec(sl.n)); };
            {
                TopTools_IndexedDataMapOfShapeListOfShape e2f, v2e;
                TopExp::MapShapesAndAncestors(src, TopAbs_EDGE, TopAbs_FACE, e2f);
                TopExp::MapShapesAndAncestors(src, TopAbs_VERTEX, TopAbs_EDGE, v2e);
                for (int i = 1; i <= fm.Extent(); i++) {
                    TopoDS_Face f = TopoDS::Face(fm(i));
                    BRepAdaptor_Surface as(f);
                    if (as.GetType() != GeomAbs_Plane) continue;
                    TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
                    TopoDS_Wire ow = BRepTools::OuterWire(fw);
                    std::vector<TopoDS_Wire> inner;
                    for (TopExp_Explorer x(fw, TopAbs_WIRE); x.More(); x.Next())
                        if (!x.Current().IsSame(ow)) inner.push_back(TopoDS::Wire(x.Current()));
                    if (inner.empty()) continue;
                    bool moved = false, whole = true, held = false;
                    std::vector<gp_Pnt> pts;
                    for (BRepTools_WireExplorer x(ow, fw); x.More(); x.Next()) {
                        TopoDS_Vertex cv = x.CurrentVertex();
                        moved = moved || vnew.IsBound(cv);
                        whole = whole && vnew.IsBound(cv);
                        pts.push_back(vnew.IsBound(cv) ? BRep_Tool::Pnt(TopoDS::Vertex(vnew(cv))) : BRep_Tool::Pnt(cv));
                    }
                    for (const TopoDS_Wire& w : inner)
                        for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next()) held = held || vnew.IsBound(x.Current());
                    // (a hole's point moved on its own: refused below)
                    if (!moved || (held && !whole) || pts.size() < 3) continue;
                    gp_XYZ nrm(0, 0, 0), cen(0, 0, 0);
                    for (size_t k = 0; k < pts.size(); k++) {
                        const gp_Pnt& p = pts[k];
                        const gp_Pnt& q = pts[(k + 1) % pts.size()];
                        nrm += gp_XYZ((p.Y() - q.Y()) * (p.Z() + q.Z()), (p.Z() - q.Z()) * (p.X() + q.X()), (p.X() - q.X()) * (p.Y() + q.Y()));
                        cen += p.XYZ();
                    }
                    if (nrm.Modulus() < 1e-12) continue;
                    cen /= (double)pts.size();
                    gp_Dir nd(nrm);
                    double off = 0;
                    for (const gp_Pnt& p : pts) off = std::max(off, std::abs((p.XYZ() - cen).Dot(nd.XYZ())));
                    if (off > 1e-6 * size + 1e-6) continue; // it bends: refused below
                    gp_Pln old = as.Plane();
                    if (std::abs(std::abs(old.Axis().Direction().Dot(nd)) - 1) < 1e-9 && old.Distance(gp_Pnt(cen)) < 1e-6 * size + 1e-7) continue;
                    // the whole face moved (a Move face: its holes' points
                    // with it): they slide along their walls instead
                    if (held)
                        for (const TopoDS_Wire& w : inner)
                            for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next()) vnew.UnBind(x.Current());
                    for (const TopoDS_Wire& w : inner) {
                        // its walls and the way they run
                        std::vector<TopoDS_Face> wf;
                        gp_Vec d(0, 0, 0);
                        std::vector<gp_Dir> pn;
                        bool ok = true;
                        for (TopExp_Explorer x(w, TopAbs_EDGE); x.More(); x.Next()) {
                            const TopTools_ListOfShape& fl = e2f.FindFromKey(x.Current());
                            for (TopTools_ListIteratorOfListOfShape it(fl); it.More(); it.Next()) {
                                if (it.Value().IsSame(f)) continue;
                                TopoDS_Face g = TopoDS::Face(it.Value());
                                wf.push_back(g);
                                BRepAdaptor_Surface ws(g);
                                if (ws.GetType() == GeomAbs_Cylinder) {
                                    if (d.Magnitude() == 0) d = gp_Vec(ws.Cylinder().Axis().Direction());
                                } else if (ws.GetType() == GeomAbs_SurfaceOfExtrusion) {
                                    if (d.Magnitude() == 0) d = gp_Vec(ws.Direction());
                                } else if (ws.GetType() == GeomAbs_Plane) {
                                    pn.push_back(ws.Plane().Axis().Direction());
                                } else {
                                    ok = false;
                                }
                            }
                        }
                        for (size_t a = 0; ok && d.Magnitude() == 0 && a < pn.size(); a++)
                            for (size_t b = a + 1; b < pn.size(); b++) {
                                gp_Vec c = gp_Vec(pn[a]).Crossed(gp_Vec(pn[b]));
                                if (c.Magnitude() > 1e-3) {
                                    d = c.Normalized();
                                    break;
                                }
                            }
                        if (d.Magnitude() == 0) ok = false;
                        for (size_t k = 0; ok && k < wf.size(); k++) {
                            BRepAdaptor_Surface ws(wf[k]);
                            if (ws.GetType() == GeomAbs_Cylinder) ok = gp_Vec(ws.Cylinder().Axis().Direction()).Crossed(d).Magnitude() < 1e-6;
                            else if (ws.GetType() == GeomAbs_SurfaceOfExtrusion) ok = gp_Vec(ws.Direction()).Crossed(d).Magnitude() < 1e-6;
                            else ok = std::abs(gp_Vec(ws.Plane().Axis().Direction()).Dot(d)) < 1e-6;
                        }
                        // (and not along either plane)
                        if (ok) ok = std::abs(d.Dot(gp_Vec(nd))) > 0.2 && std::abs(d.Dot(gp_Vec(old.Axis().Direction()))) > 0.2;
                        if (!ok) {
                            set_err("a face with a hole or a boss tilts only when their walls run straight (a rounded or slanted wall cannot follow yet)");
                            return nullptr;
                        }
                        int si = (int)slides.size();
                        slides.push_back(Slide{gp_Pnt(cen), nd, d});
                        const Slide& sl = slides.back();
                        for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next()) {
                            if (vnew.IsBound(x.Current())) continue;
                            gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(x.Current()));
                            vnew.Bind(x.Current(), BRepBuilderAPI_MakeVertex(p.Translated(d * slide_s(sl, p))).Vertex());
                            slide_of.Bind(x.Current(), si);
                        }
                        // its edges on the face carried along d
                        for (TopExp_Explorer x(w, TopAbs_EDGE); x.More(); x.Next()) {
                            TopoDS_Edge ef = TopoDS::Edge(x.Current().Oriented(TopAbs_FORWARD));
                            if (enew.IsBound(ef)) continue;
                            TopoDS_Vertex a, b;
                            TopExp::Vertices(ef, a, b);
                            TopoDS_Vertex na = TopoDS::Vertex(vnew(a).Oriented(TopAbs_FORWARD)), nb = TopoDS::Vertex(vnew(b).Oriented(TopAbs_FORWARD));
                            BRepAdaptor_Curve c(ef);
                            TopoDS_Edge ne;
                            if (c.GetType() == GeomAbs_Line) {
                                BRepBuilderAPI_MakeEdge me(na, nb);
                                if (me.IsDone()) ne = me.Edge();
                            } else {
                                // sampled at the old curve's parameters: the
                                // new curve and its (u, v) on the wall share them
                                const int N = 64;
                                double t0 = c.FirstParameter(), t1 = c.LastParameter();
                                opencascade::handle<TColgp_HArray1OfPnt> hp = new TColgp_HArray1OfPnt(1, N + 1);
                                opencascade::handle<TColStd_HArray1OfReal> hk = new TColStd_HArray1OfReal(1, N + 1);
                                for (int k = 0; k <= N; k++) {
                                    double t = t0 + (t1 - t0) * k / N;
                                    gp_Pnt p = c.Value(t);
                                    hp->SetValue(k + 1, p.Translated(d * slide_s(sl, p)));
                                    hk->SetValue(k + 1, t);
                                }
                                GeomAPI_Interpolate gi(hp, hk, Standard_False, 1e-9);
                                // its ends' tangents the old ones slid (a closed
                                // edge stays smooth where it closes)
                                auto slid_tan = [&](double t) {
                                    gp_Pnt p;
                                    gp_Vec v;
                                    c.D1(t, p, v);
                                    return v - d * (v.Dot(gp_Vec(sl.n)) / d.Dot(gp_Vec(sl.n)));
                                };
                                gi.Load(slid_tan(t0), slid_tan(t1), Standard_False);
                                gi.Perform();
                                if (gi.IsDone()) {
                                    BRepBuilderAPI_MakeEdge me(gi.Curve(), na, nb, t0, t1);
                                    if (me.IsDone()) ne = me.Edge();
                                }
                            }
                            if (ne.IsNull()) {
                                set_err("a hole's or a boss's edge could not follow the tilt");
                                return nullptr;
                            }
                            enew.Bind(ef, ne);
                            slid_e.Add(ef);
                        }
                        // the edges running down its wall keep their way
                        for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next())
                            for (TopTools_ListIteratorOfListOfShape it(v2e.FindFromKey(x.Current())); it.More(); it.Next()) {
                                if (slid_e.Contains(it.Value())) continue;
                                TopoDS_Vertex a, b;
                                TopExp::Vertices(TopoDS::Edge(it.Value()), a, b);
                                gp_Pnt pa = BRep_Tool::Pnt(a), pb = BRep_Tool::Pnt(b);
                                gp_Pnt qa = vnew.IsBound(a) ? BRep_Tool::Pnt(TopoDS::Vertex(vnew(a))) : pa;
                                gp_Pnt qb = vnew.IsBound(b) ? BRep_Tool::Pnt(TopoDS::Vertex(vnew(b))) : pb;
                                if (qa.Distance(qb) < 1e-4 || gp_Vec(qa, qb).Dot(gp_Vec(pa, pb)) <= 0) {
                                    set_err("the tilt goes past the end of a hole or a boss");
                                    return nullptr;
                                }
                            }
                        for (const TopoDS_Face& g : wf) walls.Add(g);
                        tilted.Add(f);
                    }
                }
            }
            // curved edges whose ends move (20 nov.): carried whole when
            // both ends move by one vector (a fillet's end on a face moved
            // along its axis), else kept on their curve when each moved end
            // stays on it (a cut's end sliding along a fillet's edge)
            {
                TopTools_IndexedMapOfShape em;
                TopExp::MapShapes(src, TopAbs_EDGE, em);
                const double tol = 1e-6 * size + 1e-6;
                for (int i = 1; i <= em.Extent(); i++) {
                    TopoDS_Edge ef = TopoDS::Edge(em(i).Oriented(TopAbs_FORWARD));
                    if (enew.IsBound(ef) || BRep_Tool::Degenerated(ef)) continue;
                    TopoDS_Vertex a, b;
                    TopExp::Vertices(ef, a, b);
                    bool ma = vnew.IsBound(a), mb = vnew.IsBound(b);
                    if (!ma && !mb) continue;
                    if (BRepAdaptor_Curve(ef).GetType() == GeomAbs_Line) continue;
                    double t0, t1;
                    opencascade::handle<Geom_Curve> gc = BRep_Tool::Curve(ef, t0, t1);
                    if (gc.IsNull()) continue;
                    TopoDS_Vertex na = ma ? TopoDS::Vertex(vnew(a)) : a, nb = mb ? TopoDS::Vertex(vnew(b)) : b;
                    gp_Pnt pa = BRep_Tool::Pnt(a), pb = BRep_Tool::Pnt(b), qa = BRep_Tool::Pnt(na), qb = BRep_Tool::Pnt(nb);
                    gp_Vec va(pa, qa), vb(pb, qb);
                    TopoDS_Edge ne;
                    if (ma && mb && (va - vb).Magnitude() < tol) {
                        gp_Trsf tr;
                        tr.SetTranslation(va);
                        opencascade::handle<Geom_Curve> mc = opencascade::handle<Geom_Curve>::DownCast(gc->Transformed(tr));
                        BRepBuilderAPI_MakeEdge me(mc, TopoDS::Vertex(na.Oriented(TopAbs_FORWARD)), TopoDS::Vertex(nb.Oriented(TopAbs_FORWARD)), t0, t1);
                        if (me.IsDone()) ne = me.Edge();
                    } else if (!a.IsSame(b)) {
                        // each moved end on the curve: its parameter there
                        auto param = [&](const gp_Pnt& q, double old, double& t) {
                            GeomAPI_ProjectPointOnCurve pj(q, gc);
                            if (pj.NbPoints() == 0 || pj.LowerDistance() > tol) return false;
                            t = pj.LowerDistanceParameter();
                            if (gc->IsPeriodic()) {
                                double per = gc->Period();
                                while (t - old > per / 2) t -= per;
                                while (old - t > per / 2) t += per;
                            }
                            return true;
                        };
                        double n0 = t0, n1 = t1;
                        if ((ma && !param(qa, t0, n0)) || (mb && !param(qb, t1, n1))) continue;
                        if (n1 - n0 < 1e-9) continue;
                        BRepBuilderAPI_MakeEdge me(gc, TopoDS::Vertex(na.Oriented(TopAbs_FORWARD)), TopoDS::Vertex(nb.Oriented(TopAbs_FORWARD)), n0, n1);
                        if (me.IsDone()) ne = me.Edge();
                    }
                    if (!ne.IsNull()) enew.Bind(ef, ne);
                }
            }
            // one new shell per shell of the part, one solid per solid (a
            // part of several lumps: a cube added apart, 20 nov.)
            TopTools_IndexedMapOfShape shm;
            TopExp::MapShapes(src, TopAbs_SHELL, shm);
            std::vector<TopoDS_Shell> shells(std::max(1, shm.Extent()));
            for (TopoDS_Shell& sh : shells) bb.MakeShell(sh);
            TopTools_DataMapOfShapeInteger shell_of;
            for (int k = 1; k <= shm.Extent(); k++)
                for (TopExp_Explorer x(shm(k), TopAbs_FACE); x.More(); x.Next())
                    if (!shell_of.IsBound(x.Current())) shell_of.Bind(x.Current(), k - 1);
            auto shell_for = [&](const TopoDS_Shape& f) -> TopoDS_Shell& { return shells[shell_of.IsBound(f) ? shell_of(f) : 0]; };
            std::vector<long long> tags;
            for (int i = 1; i <= fm.Extent(); i++) {
                TopoDS_Face f = TopoDS::Face(fm(i));
                long long tag = i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
                bool touched = false, slides_here = false;
                for (TopExp_Explorer x(f, TopAbs_VERTEX); x.More(); x.Next()) {
                    touched = touched || vnew.IsBound(x.Current());
                    slides_here = slides_here || slide_of.IsBound(x.Current());
                }
                if (!touched) {
                    bb.Add(shell_for(f), f);
                    tags.push_back(tag);
                    continue;
                }
                if (slides_here && !walls.Contains(f) && !tilted.Contains(f)) {
                    set_err("a hole or a boss next to the moved element cannot follow yet");
                    return nullptr;
                }
                BRepAdaptor_Surface as(f);
                if (walls.Contains(f) && as.GetType() != GeomAbs_Plane) {
                    // a curved wall of a hole or a boss on a tilted face:
                    // its surface kept, its edges on the face and those
                    // running along it remade, their (u, v) the old ones
                    // moved along v as far as their points slid
                    TopLoc_Location loc;
                    opencascade::handle<Geom_Surface> gs = BRep_Tool::Surface(f, loc);
                    gp_Trsf tr = loc.Transformation();
                    TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
                    TopoDS_Face nf;
                    bb.MakeFace(nf, gs, loc, BRep_Tool::Tolerance(f));
                    auto lift = [&](const gp_Pnt2d& uv, const Slide& sl) {
                        gp_Pnt p;
                        gp_Vec du, dv;
                        gs->D1(uv.X(), uv.Y(), p, du, dv);
                        p.Transform(tr);
                        dv.Transform(tr);
                        return gp_Pnt2d(uv.X(), uv.Y() + slide_s(sl, p) * sl.d.Dot(dv) / dv.SquareMagnitude());
                    };
                    TopTools_MapOfShape done;
                    for (TopoDS_Iterator wi(fw); wi.More(); wi.Next()) {
                        TopoDS_Wire ow = TopoDS::Wire(wi.Value());
                        TopoDS_Wire nw;
                        bb.MakeWire(nw);
                        for (TopoDS_Iterator ei(ow.Oriented(TopAbs_FORWARD)); ei.More(); ei.Next()) {
                            TopoDS_Edge e = TopoDS::Edge(ei.Value());
                            TopoDS_Edge ef = TopoDS::Edge(e.Oriented(TopAbs_FORWARD));
                            TopoDS_Vertex a, b;
                            TopExp::Vertices(ef, a, b);
                            bool ma = vnew.IsBound(a), mb = vnew.IsBound(b);
                            if (!ma && !mb) {
                                bb.Add(nw, e);
                                continue;
                            }
                            if ((ma && !slide_of.IsBound(a)) || (mb && !slide_of.IsBound(b))) {
                                set_err("a hole or a boss next to the moved element cannot follow yet");
                                return nullptr;
                            }
                            TopoDS_Edge ne;
                            if (enew.IsBound(ef)) {
                                ne = TopoDS::Edge(enew(ef));
                            } else {
                                BRepAdaptor_Curve c(ef);
                                TopoDS_Vertex na = ma ? TopoDS::Vertex(vnew(a)) : a;
                                TopoDS_Vertex nb = mb ? TopoDS::Vertex(vnew(b)) : b;
                                if (c.GetType() != GeomAbs_Line || BRep_Tool::Pnt(na).Distance(BRep_Tool::Pnt(nb)) < 1e-4) {
                                    set_err("a hole's or a boss's wall could not follow the tilt");
                                    return nullptr;
                                }
                                BRepBuilderAPI_MakeEdge me(TopoDS::Vertex(na.Oriented(TopAbs_FORWARD)), TopoDS::Vertex(nb.Oriented(TopAbs_FORWARD)));
                                if (!me.IsDone()) {
                                    set_err("a hole's or a boss's wall could not follow the tilt");
                                    return nullptr;
                                }
                                ne = me.Edge();
                                enew.Bind(ef, ne);
                            }
                            bb.Add(nw, TopoDS::Edge(ne.Oriented(e.Orientation())));
                            if (done.Contains(ne)) continue;
                            done.Add(ne);
                            // its curve(s) on the wall (a seam has two)
                            double g0, g1, n0, n1;
                            BRep_Tool::Range(ne, n0, n1);
                            bool seam = BRep_Tool::IsClosed(ef, fw);
                            bool line = BRepAdaptor_Curve(ne).GetType() == GeomAbs_Line;
                            auto moved_pc = [&](const opencascade::handle<Geom2d_Curve>& pc) -> opencascade::handle<Geom2d_Curve> {
                                if (pc.IsNull()) return nullptr;
                                if (slid_e.Contains(ef) && !line) {
                                    // sampled at the parameters the new edge shares
                                    const int N = 64;
                                    const Slide& sl = slides[slide_of(a)];
                                    opencascade::handle<TColgp_HArray1OfPnt2d> hp = new TColgp_HArray1OfPnt2d(1, N + 1);
                                    opencascade::handle<TColStd_HArray1OfReal> hk = new TColStd_HArray1OfReal(1, N + 1);
                                    for (int k = 0; k <= N; k++) {
                                        double t = n0 + (n1 - n0) * k / N;
                                        hp->SetValue(k + 1, lift(pc->Value(t), sl));
                                        hk->SetValue(k + 1, t);
                                    }
                                    Geom2dAPI_Interpolate gi(hp, hk, Standard_False, 1e-9);
                                    // the ends' tangents: the old curve's, v moved
                                    // as fast as the slide changes along it
                                    BRepAdaptor_Curve oc(ef);
                                    auto tan2 = [&](double t) {
                                        gp_Pnt2d uv;
                                        gp_Vec2d w;
                                        pc->D1(t, uv, w);
                                        gp_Pnt p, q;
                                        gp_Vec c1, du, dv;
                                        oc.D1(t, p, c1);
                                        gs->D1(uv.X(), uv.Y(), q, du, dv);
                                        dv.Transform(tr);
                                        double ds = -c1.Dot(gp_Vec(sl.n)) / sl.d.Dot(gp_Vec(sl.n));
                                        return gp_Vec2d(w.X(), w.Y() + ds * sl.d.Dot(dv) / dv.SquareMagnitude());
                                    };
                                    gi.Load(tan2(n0), tan2(n1), Standard_False);
                                    gi.Perform();
                                    if (!gi.IsDone()) return nullptr;
                                    return opencascade::handle<Geom2d_Curve>(gi.Curve());
                                }
                                // a line: end to end
                                gp_Pnt2d ua = pc->Value(BRep_Tool::Parameter(a, ef)), ub = pc->Value(BRep_Tool::Parameter(b, ef));
                                if (ma) ua = lift(ua, slides[slide_of(a)]);
                                if (mb) ub = lift(ub, slides[slide_of(b)]);
                                TColgp_Array1OfPnt2d pp(1, 2);
                                pp(1) = ua;
                                pp(2) = ub;
                                TColStd_Array1OfReal kn(1, 2);
                                kn(1) = n0;
                                kn(2) = n1;
                                TColStd_Array1OfInteger mu(1, 2);
                                mu(1) = 2;
                                mu(2) = 2;
                                return opencascade::handle<Geom2d_Curve>(new Geom2d_BSplineCurve(pp, kn, mu, 1));
                            };
                            opencascade::handle<Geom2d_Curve> c1 = moved_pc(BRep_Tool::CurveOnSurface(ef, fw, g0, g1));
                            opencascade::handle<Geom2d_Curve> c2 = seam ? moved_pc(BRep_Tool::CurveOnSurface(TopoDS::Edge(ef.Reversed()), fw, g0, g1)) : nullptr;
                            if (c1.IsNull() || (seam && c2.IsNull())) {
                                set_err("a hole's or a boss's wall could not follow the tilt");
                                return nullptr;
                            }
                            if (seam) bb.UpdateEdge(TopoDS::Edge(ne.Oriented(TopAbs_FORWARD)), c1, c2, nf, BRep_Tool::Tolerance(ne));
                            else bb.UpdateEdge(ne, c1, nf, BRep_Tool::Tolerance(ne));
                        }
                        nw.Closed(Standard_True);
                        nw.Orientation(ow.Orientation());
                        bb.Add(nf, nw);
                    }
                    // the new edges' tolerance to cover their curves' gap
                    BRepAdaptor_Surface an(nf);
                    for (TopExp_Explorer x(nf, TopAbs_EDGE); x.More(); x.Next()) {
                        TopoDS_Edge e = TopoDS::Edge(x.Current());
                        if (!done.Contains(e)) continue;
                        double g0, g1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, nf, g0, g1);
                        if (pc.IsNull()) continue;
                        BRepAdaptor_Curve ec(e);
                        double dmax = 0;
                        for (int k = 0; k <= 160; k++) {
                            double t = ec.FirstParameter() + (ec.LastParameter() - ec.FirstParameter()) * k / 160.0;
                            gp_Pnt2d uv = pc->Value(g0 + (g1 - g0) * k / 160.0);
                            dmax = std::max(dmax, an.Value(uv.X(), uv.Y()).Distance(ec.Value(t)));
                        }
                        if (dmax > 1e-2 * size) {
                            set_err("a hole's or a boss's wall could not follow the tilt");
                            return nullptr;
                        }
                        if (dmax > BRep_Tool::Tolerance(e)) bb.UpdateEdge(e, 1.5 * dmax);
                    }
                    nf.Orientation(f.Orientation());
                    bb.Add(shell_for(f), nf);
                    tags.push_back(tag);
                    continue;
                }
                if (as.GetType() != GeomAbs_Plane) {
                    // a curved face round the move (20 nov.: a fillet's face
                    // whose edge a cut's end slides along, a fillet's end
                    // carried along its axis): it keeps its surface when
                    // the moved points and edges stay on it
                    TopLoc_Location loc;
                    opencascade::handle<Geom_Surface> gs = BRep_Tool::Surface(f, loc);
                    opencascade::handle<Geom_Surface> gl = BRep_Tool::Surface(f);
                    const double tol = 1e-5 * size + 1e-6;
                    const char* why = "only flat faces can follow a move for now (a curved face next to it would have to change)";
                    auto on = [&](const gp_Pnt& p) {
                        GeomAPI_ProjectPointOnSurf pj(p, gl);
                        return pj.NbPoints() > 0 && pj.LowerDistance() <= tol;
                    };
                    TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
                    TopoDS_Face nf;
                    bb.MakeFace(nf, gs, loc, BRep_Tool::Tolerance(f));
                    std::vector<TopoDS_Edge> fresh;
                    for (TopoDS_Iterator wi(fw); wi.More(); wi.Next()) {
                        TopoDS_Wire ow = TopoDS::Wire(wi.Value());
                        TopoDS_Wire nw;
                        bb.MakeWire(nw);
                        for (TopoDS_Iterator ei(ow.Oriented(TopAbs_FORWARD)); ei.More(); ei.Next()) {
                            TopoDS_Edge e = TopoDS::Edge(ei.Value());
                            TopoDS_Edge ef = TopoDS::Edge(e.Oriented(TopAbs_FORWARD));
                            TopoDS_Vertex a, b;
                            TopExp::Vertices(ef, a, b);
                            bool ma = vnew.IsBound(a), mb = vnew.IsBound(b);
                            if (!ma && !mb) {
                                bb.Add(nw, e);
                                continue;
                            }
                            if (BRep_Tool::IsClosed(ef, fw)) {
                                set_err(why);
                                return nullptr;
                            }
                            TopoDS_Edge ne;
                            if (enew.IsBound(ef)) {
                                ne = TopoDS::Edge(enew(ef));
                            } else if (BRepAdaptor_Curve(ef).GetType() == GeomAbs_Line) {
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
                                enew.Bind(ef, ne);
                            } else {
                                set_err("only straight edges can follow a move for now");
                                return nullptr;
                            }
                            BRepAdaptor_Curve nc(ne);
                            for (int k = 0; k <= 8; k++)
                                if (!on(nc.Value(nc.FirstParameter() + (nc.LastParameter() - nc.FirstParameter()) * k / 8.0))) {
                                    set_err(why);
                                    return nullptr;
                                }
                            bb.Add(nw, TopoDS::Edge(ne.Oriented(e.Orientation())));
                            fresh.push_back(ne);
                        }
                        nw.Closed(Standard_True);
                        nw.Orientation(ow.Orientation());
                        bb.Add(nf, nw);
                    }
                    for (const TopoDS_Edge& e : fresh) {
                        ShapeFix_Edge sfe;
                        sfe.FixAddPCurve(e, nf, Standard_False, 1e-6);
                        double g0, g1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, nf, g0, g1);
                        if (pc.IsNull()) {
                            set_err(why);
                            return nullptr;
                        }
                        BRepAdaptor_Curve ec(e);
                        BRepAdaptor_Surface an(nf, Standard_False);
                        double dmax = 0;
                        for (int k = 0; k <= 160; k++) {
                            double t = ec.FirstParameter() + (ec.LastParameter() - ec.FirstParameter()) * k / 160.0;
                            gp_Pnt2d uv = pc->Value(g0 + (g1 - g0) * k / 160.0);
                            dmax = std::max(dmax, an.Value(uv.X(), uv.Y()).Distance(ec.Value(t)));
                        }
                        if (dmax > 1e-3 * size) {
                            set_err(why);
                            return nullptr;
                        }
                        if (dmax > BRep_Tool::Tolerance(e)) bb.UpdateEdge(e, 1.5 * dmax);
                    }
                    nf.Orientation(f.Orientation());
                    bb.Add(shell_for(f), nf);
                    tags.push_back(tag);
                    continue;
                }
                TopoDS_Face fw = TopoDS::Face(f.Oriented(TopAbs_FORWARD));
                TopoDS_Wire ow = BRepTools::OuterWire(fw);
                std::vector<TopoDS_Wire> inner;
                for (TopExp_Explorer x(fw, TopAbs_WIRE); x.More(); x.Next())
                    if (!x.Current().IsSame(ow)) inner.push_back(TopoDS::Wire(x.Current()));
                // its holes and bosses: as they were, or slid onto its new
                // plane (their edges remade before)
                std::vector<TopoDS_Wire> ninner;
                bool kept_inner = false;
                for (const TopoDS_Wire& w : inner) {
                    bool slid = false;
                    for (TopExp_Explorer x(w, TopAbs_VERTEX); x.More(); x.Next()) {
                        if (vnew.IsBound(x.Current()) && !slide_of.IsBound(x.Current())) {
                            set_err("a hole or a boss next to the moved element cannot follow yet");
                            return nullptr;
                        }
                        slid = slid || slide_of.IsBound(x.Current());
                    }
                    if (!slid) {
                        kept_inner = true;
                        ninner.push_back(w);
                        continue;
                    }
                    TopoDS_Wire sw;
                    bb.MakeWire(sw);
                    for (TopoDS_Iterator it(w.Oriented(TopAbs_FORWARD)); it.More(); it.Next()) {
                        TopoDS_Edge e = TopoDS::Edge(it.Value());
                        if (!enew.IsBound(e)) {
                            set_err("a hole or a boss could not follow the tilt");
                            return nullptr;
                        }
                        bb.Add(sw, TopoDS::Edge(enew(e).Oriented(e.Orientation())));
                    }
                    sw.Closed(Standard_True);
                    sw.Orientation(w.Orientation());
                    ninner.push_back(sw);
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
                    if (c.GetType() != GeomAbs_Line && !enew.IsBound(e)) {
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
                bool all_lines = true;
                for (TopExp_Explorer x(nw, TopAbs_EDGE); x.More() && all_lines; x.Next()) all_lines = BRepAdaptor_Curve(TopoDS::Edge(x.Current())).GetType() == GeomAbs_Line;
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
                    if (kept_inner && !same) {
                        set_err("a face with a hole cannot tilt or move off its plane yet");
                        return nullptr;
                    }
                    if (same) pl = gp_Pln(old.Location(), old.Axis().Direction().Dot(nd) > 0 ? old.Axis().Direction() : old.Axis().Direction().Reversed());
                    // a curved edge kept as it was must lie on the new plane
                    // (an arc between two corners the tilt turns round)
                    if (!same) {
                        std::vector<TopoDS_Wire> all = ninner;
                        all.push_back(nw);
                        for (const TopoDS_Wire& w : all)
                            for (TopExp_Explorer x(w, TopAbs_EDGE); x.More(); x.Next()) {
                                BRepAdaptor_Curve c(TopoDS::Edge(x.Current()));
                                for (int k = 1; k < 8; k++)
                                    if (pl.Distance(c.Value(c.FirstParameter() + (c.LastParameter() - c.FirstParameter()) * k / 8.0)) > 1e-5 * size + 1e-6) {
                                        set_err("a curved edge of the face cannot follow its tilt yet");
                                        return nullptr;
                                    }
                            }
                    }
                    BRepBuilderAPI_MakeFace mf(pl, nw, Standard_True);
                    for (const TopoDS_Wire& w : ninner) mf.Add(w);
                    if (!mf.IsDone()) {
                        set_err("a face could not follow the move");
                        return nullptr;
                    }
                    nf = mf.Face();
                } else if (pts.size() == 4 && inner.empty() && all_lines) {
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
                    // a bent face of more sides (a block's side after a
                    // chamfer, a sketch's top): a smooth surface through its
                    // edges (a plate, made a spline), each edge put on it
                    GeomPlate_BuildPlateSurface bp(3, 15, 2);
                    for (TopExp_Explorer x(nw, TopAbs_EDGE); x.More(); x.Next()) {
                        opencascade::handle<BRepAdaptor_Curve> c = new BRepAdaptor_Curve(TopoDS::Edge(x.Current()));
                        bp.Add(new GeomPlate_CurveConstraint(c, 0, 24));
                    }
                    bp.Perform();
                    if (!bp.IsDone()) {
                        set_err("a bent face could not follow the move");
                        return nullptr;
                    }
                    GeomPlate_MakeApprox ap(bp.Surface(), 1e-6, 1, 8, std::max(1e-5, 2.0 * bp.G0Error()), 0);
                    opencascade::handle<Geom_BSplineSurface> ps = ap.Surface();
                    if (ps.IsNull()) {
                        set_err("a bent face could not follow the move");
                        return nullptr;
                    }
                    // its normal the outline's way (Newell's)
                    GeomAPI_ProjectPointOnSurf pj(gp_Pnt(cen), ps);
                    if (pj.NbPoints() > 0) {
                        double pu, pv;
                        pj.LowerDistanceParameters(pu, pv);
                        gp_Pnt q;
                        gp_Vec du, dv;
                        ps->D1(pu, pv, q, du, dv);
                        if (du.Crossed(dv).Dot(gp_Vec(nd)) < 0) ps->UReverse();
                    }
                    bb.MakeFace(nf, ps, Precision::Confusion());
                    bb.Add(nf, nw);
                    for (TopExp_Explorer x(nf, TopAbs_EDGE); x.More(); x.Next()) {
                        TopoDS_Edge e = TopoDS::Edge(x.Current());
                        ShapeFix_Edge sfe;
                        sfe.FixAddPCurve(e, nf, Standard_False, 1e-6);
                        // its tolerance to cover how far the plate passes
                        double g0, g1;
                        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, nf, g0, g1);
                        if (pc.IsNull()) {
                            set_err("a bent face could not follow the move");
                            return nullptr;
                        }
                        BRepAdaptor_Curve ec(e);
                        double dmax = 0;
                        for (int k = 0; k <= 160; k++) {
                            double t = ec.FirstParameter() + (ec.LastParameter() - ec.FirstParameter()) * k / 160.0;
                            gp_Pnt2d uv = pc->Value(g0 + (g1 - g0) * k / 160.0);
                            dmax = std::max(dmax, ps->Value(uv.X(), uv.Y()).Distance(ec.Value(t)));
                        }
                        if (dmax > 1e-2 * size) {
                            set_err("a bent face could not follow the move");
                            return nullptr;
                        }
                        if (dmax > BRep_Tool::Tolerance(e)) bb.UpdateEdge(e, 1.5 * dmax);
                    }
                }
                nf.Orientation(f.Orientation());
                bb.Add(shell_for(f), nf);
                tags.push_back(tag);
            }
            // the solids again, each with its shells
            std::vector<TopoDS_Solid> solids;
            TopTools_MapOfShape placed;
            auto shell_new = [&](const TopoDS_Shape& old) -> TopoDS_Shell& { return shells[shm.FindIndex(old) - 1]; };
            for (TopExp_Explorer x(src, TopAbs_SOLID); x.More(); x.Next()) {
                TopoDS_Solid so;
                bb.MakeSolid(so);
                for (TopExp_Explorer y(x.Current(), TopAbs_SHELL); y.More(); y.Next()) {
                    if (placed.Contains(y.Current())) continue;
                    placed.Add(y.Current());
                    TopoDS_Shell& sh = shell_new(y.Current());
                    sh.Closed(BRep_Tool::IsClosed(sh));
                    bb.Add(so, sh);
                }
                BRepLib::OrientClosedSolid(so);
                solids.push_back(so);
            }
            // a shell of no solid: a solid when it closes, else it stays an
            // open sheet (a copy of some faces, 20 nov.: made a solid, it was
            // invalid and no face or edge of the copy could be moved)
            std::vector<TopoDS_Shape> sheets;
            auto place = [&](TopoDS_Shell& sh) {
                bool closed = BRep_Tool::IsClosed(sh);
                sh.Closed(closed);
                if (!closed) {
                    sheets.push_back(sh);
                    return;
                }
                TopoDS_Solid so;
                bb.MakeSolid(so);
                bb.Add(so, sh);
                BRepLib::OrientClosedSolid(so);
                solids.push_back(so);
            };
            for (int k = 1; k <= shm.Extent(); k++)
                if (!placed.Contains(shm(k))) place(shells[k - 1]);
            if (shm.Extent() == 0) place(shells[0]);
            TopoDS_Shape solid;
            if (solids.size() == 1 && sheets.empty()) {
                solid = solids[0];
            } else {
                TopoDS_Compound c;
                bb.MakeCompound(c);
                for (const TopoDS_Solid& so : solids) bb.Add(c, so);
                for (const TopoDS_Shape& sh : sheets) bb.Add(c, sh);
                solid = c;
            }
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

// Face `index`'s bounding box: xmin ymin zmin xmax ymax zmax.
int focc_face_box(const FShape* s, int index, double* out) {
    return guard(
        [&] {
            TopTools_IndexedMapOfShape m;
            face_map(s->s, m);
            if (index < 0 || index >= m.Extent()) return -1;
            Bnd_Box b;
            BRepBndLib::AddOptimal(m(index + 1), b, Standard_False, Standard_False);
            if (b.IsVoid()) return -1;
            b.Get(out[0], out[1], out[2], out[3], out[4], out[5]);
            return 0;
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



// A surface with no thickness through a closed outline of straight sides
// (`xyz`, n points), smooth inside, passing near `inner` (m points: the
// scan under it); a flat outline gives a plane.
FShape* focc_fill(const double* xyz, int n, const double* inner, int m) {
    return guard(
        [&]() -> FShape* {
            if (n < 3) {
                set_err("a surface needs three points at least");
                return nullptr;
            }
            auto P = [](const double* a) { return gp_Pnt(a[0], a[1], a[2]); };
            std::vector<TopoDS_Vertex> v(n);
            for (int i = 0; i < n; i++) v[i] = BRepBuilderAPI_MakeVertex(P(xyz + 3 * i));
            BRepFill_Filling fill(3, 15, 2, Standard_False, 1e-5, 1e-4, 1e-2, 0.1, 8, 9);
            int edges = 0;
            gp_Vec nw(0, 0, 0);
            for (int i = 0; i < n; i++) {
                int j = (i + 1) % n;
                gp_Pnt a = P(xyz + 3 * i), b = P(xyz + 3 * j);
                nw += gp_Vec(a.XYZ()).Crossed(gp_Vec(b.XYZ()));
                if (a.Distance(b) < 1e-6) continue;
                fill.Add(BRepBuilderAPI_MakeEdge(v[i], v[j]).Edge(), GeomAbs_C0, Standard_True);
                edges++;
            }
            if (edges < 3) {
                set_err("the outline has no area");
                return nullptr;
            }
            for (int k = 0; k < m; k++) fill.Add(P(inner + 3 * k));
            fill.Build();
            if (!fill.IsDone()) {
                set_err("the outline could not be filled");
                return nullptr;
            }
            TopoDS_Face f = fill.Face();
            if (f.IsNull()) {
                set_err("the outline could not be filled");
                return nullptr;
            }
            // facing the outline's way (Newell's normal)
            BRepAdaptor_Surface sa(f);
            double u = 0.5 * (sa.FirstUParameter() + sa.LastUParameter()), w = 0.5 * (sa.FirstVParameter() + sa.LastVParameter());
            gp_Pnt q;
            gp_Vec du, dv;
            sa.D1(u, w, q, du, dv);
            gp_Vec sn = du.Crossed(dv);
            if (f.Orientation() == TopAbs_REVERSED) sn.Reverse();
            if (sn.Dot(nw) < 0) f.Reverse();
            return wrap(f);
        },
        (FShape*)nullptr);
}

// A surface following a height field (the scan under an outline): in the
// frame `fr` (origin, x, y = up, z; 12 numbers), heights `h` (y) on an
// nx × nz grid from (x0, z0) to (x1, z1), approximated by a BSpline whose
// (u, v) are x and z; trimmed by the outline `poly` (n points: x, z), its
// sides straight in (x, z). Faces up (+y).
FShape* focc_sheet_grid(const double* fr, int nx, int nz, double x0, double z0, double x1, double z1, const double* h, const double* poly, int n) {
    return guard(
        [&]() -> FShape* {
            if (nx < 2 || nz < 2 || n < 3) {
                set_err("a surface needs a grid and three points");
                return nullptr;
            }
            // in the local frame (X = x, Y = z, Z = x × z = -y), u = x, v = z
            TColStd_Array2OfReal zs(1, nx, 1, nz);
            for (int i = 0; i < nx; i++)
                for (int k = 0; k < nz; k++) zs(i + 1, k + 1) = -h[i * nz + k];
            GeomAPI_PointsToBSplineSurface ap;
            // through the grid itself: a plain bicubic, one span per cell
            // (an approximation to a tolerance added knots without end on
            // the scan's noise, and the mesh went to a million triangles)
            ap.Interpolate(zs, x0, (x1 - x0) / (nx - 1), z0, (z1 - z0) / (nz - 1));
            if (!ap.IsDone()) {
                set_err("the scan under the outline could not be followed");
                return nullptr;
            }
            // a flat grid (a surface on its plane): a plane, (u, v) = (x, z)
            // too (a flat BSpline gave no mesh at all)
            double hmin = h[0], hmax = h[0];
            for (int k = 0; k < nx * nz; k++) hmin = std::min(hmin, h[k]), hmax = std::max(hmax, h[k]);
            opencascade::handle<Geom_Surface> surf;
            if (hmax - hmin < 1e-9)
                surf = new Geom_Plane(gp_Ax3(gp_Pnt(0, 0, -hmin), gp_Dir(0, 0, 1), gp_Dir(1, 0, 0)));
            else
                surf = ap.Surface();
            // the outline's corners (repeats dropped), one vertex each,
            // shared by the two sides that meet there: the wire is closed
            // by its topology, not by the points' match
            std::vector<gp_Pnt2d> uv;
            for (int i = 0; i < n; i++) {
                gp_Pnt2d q(poly[2 * i], poly[2 * i + 1]);
                if (uv.empty() || uv.back().Distance(q) > 1e-6) uv.push_back(q);
            }
            while (uv.size() > 1 && uv.front().Distance(uv.back()) <= 1e-6) uv.pop_back();
            int m = (int)uv.size();
            if (m < 3) {
                set_err("the outline could not be laid on the scan");
                return nullptr;
            }
            std::vector<TopoDS_Vertex> vs(m);
            for (int i = 0; i < m; i++) vs[i] = BRepBuilderAPI_MakeVertex(surf->Value(uv[i].X(), uv[i].Y()));
            BRep_Builder bb;
            TopoDS_Wire w;
            bb.MakeWire(w);
            for (int i = 0; i < m; i++) {
                int j = (i + 1) % m;
                opencascade::handle<Geom2d_Line> ln = new Geom2d_Line(uv[i], gp_Dir2d(gp_Vec2d(uv[i], uv[j])));
                double len = uv[i].Distance(uv[j]);
                BRepBuilderAPI_MakeEdge me(ln, surf, vs[i], vs[j], 0.0, len);
                if (!me.IsDone()) {
                    set_err("the outline could not be laid on the scan");
                    return nullptr;
                }
                bb.Add(w, me.Edge());
            }
            w.Closed(Standard_True);
            BRepBuilderAPI_MakeFace mf(surf, w, Standard_True);
            if (!mf.IsDone()) {
                set_err("the outline could not be filled");
                return nullptr;
            }
            ShapeFix_Face fix(mf.Face());
            fix.Perform();
            TopoDS_Face f = fix.Face();
            // its normal: x × z = -y here: turned to face +y
            f.Reverse();
            gp_Pnt o(fr[0], fr[1], fr[2]);
            gp_Dir dx(fr[3], fr[4], fr[5]), dz(fr[9], fr[10], fr[11]);
            gp_Dir dn = dx.Crossed(dz);
            gp_Trsf t;
            t.SetTransformation(gp_Ax3(o, dn, dx));
            t.Invert();
            BRepBuilderAPI_Transform tr(f, t, Standard_True);
            return wrap(tr.Shape());
        },
        (FShape*)nullptr);
}

// `a` and `b` side by side in one compound (no boolean), their tags kept.
FShape* focc_compound(const FShape* a, const FShape* b) {
    return guard(
        [&]() -> FShape* {
            BRep_Builder bb;
            TopoDS_Compound c;
            bb.MakeCompound(c);
            bb.Add(c, a->s);
            bb.Add(c, b->s);
            FShape* r = wrap(c);
            if (!r) return nullptr;
            for (const FShape* x : {a, b}) {
                TopTools_IndexedMapOfShape fm;
                face_map(x->s, fm);
                for (int i = 0; i < fm.Extent(); i++) r->tags.push_back(i < (int)x->tags.size() ? x->tags[i] : -1);
            }
            return r;
        },
        (FShape*)nullptr);
}


// Delete faces (Face mode + Del, Romain 19 nov. 23 h 22): the faces at
// these indices (0-based) taken out, the part left open where they were.
// The others keep their tags.
// Duplicate faces (Ctrl + D in Face mode, 20 nov.): the faces (0-based)
// copied, sewn, their open loops closed by flat faces when they can be (a
// piece of the part), then moved by the affine map `m` (3 rows of 4). The
// copies are tagged `tag | 1`, the closing faces `tag`. A piece that cannot
// be closed stays a sheet (its volume is 0).
// The solids of a shape (`solids` 1) or what is not a solid (0: sheets,
// shells, faces left beside the part), tags kept; null when there is none.
// A boolean is made on the solids alone, the sheets put back beside it
// (a union of a solid with a compound holding a sheet fails).
FShape* focc_lumps_of(const FShape* s, int solids) {
    return guard(
        [&]() -> FShape* {
            std::vector<TopoDS_Shape> keep;
            std::function<void(const TopoDS_Shape&)> flat = [&](const TopoDS_Shape& x) {
                if (x.ShapeType() == TopAbs_COMPOUND || x.ShapeType() == TopAbs_COMPSOLID) {
                    for (TopoDS_Iterator it(x); it.More(); it.Next()) flat(it.Value());
                } else if ((x.ShapeType() == TopAbs_SOLID) == (solids != 0)) {
                    keep.push_back(x);
                }
            };
            flat(s->s);
            if (keep.empty()) {
                set_err(solids ? "no solid" : "no sheet");
                return nullptr;
            }
            TopoDS_Shape res;
            if (keep.size() == 1) {
                res = keep[0];
            } else {
                BRep_Builder bb;
                TopoDS_Compound c;
                bb.MakeCompound(c);
                for (const TopoDS_Shape& k : keep) bb.Add(c, k);
                res = c;
            }
            TopTools_IndexedMapOfShape of, rf;
            face_map(s->s, of);
            face_map(res, rf);
            std::vector<long long> tags(rf.Extent(), -1);
            for (int i = 1; i <= rf.Extent(); i++) {
                int k = of.FindIndex(rf(i));
                if (k > 0 && k - 1 < (int)s->tags.size()) tags[i - 1] = s->tags[k - 1];
            }
            FShape* r = wrap(res);
            if (!r) return nullptr;
            r->tags = tags;
            return r;
        },
        (FShape*)nullptr);
}

// Whether a 3 x 4 matrix (rows) is a turn and a move with one scale (a
// gp_Trsf, which keeps planes planes and circles circles). gp_GTrsf's Form()
// is always gp_Other once its vectorial part is set, and GTransform makes
// every surface a BSpline (20 nov.: a copy moved apart then had no flat
// face, so none of its faces or edges could be moved).
static bool similarity(const double* m) {
    gp_Vec c0(m[0], m[4], m[8]), c1(m[1], m[5], m[9]), c2(m[2], m[6], m[10]);
    double s = c0.SquareMagnitude();
    if (s < 1e-24) return false;
    // (not `near`: a macro on Windows)
    auto same = [&](double a, double b) { return std::abs(a - b) <= 1e-9 * s; };
    return same(c1.SquareMagnitude(), s) && same(c2.SquareMagnitude(), s) && same(c0.Dot(c1), 0) && same(c0.Dot(c2), 0) && same(c1.Dot(c2), 0) && c0.Crossed(c1).Dot(c2) > 0;
}

// The lumps (solids, shells or faces side by side in a compound) whose
// vertices are all among `vidx` (1-based) moved whole by the 3 × 4 matrix
// `m` (rows); the others kept. Null when a lump has only some of its
// vertices there (a deformation: focc_move_vertices) or none moves.
FShape* focc_move_lumps(const FShape* s, const int* vidx, int n, const double* m) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape vm;
            TopExp::MapShapes(s->s, TopAbs_VERTEX, vm);
            TopTools_MapOfShape moved;
            for (int i = 0; i < n; i++) {
                if (vidx[i] < 1 || vidx[i] > vm.Extent()) {
                    set_err("no such vertex");
                    return nullptr;
                }
                moved.Add(vm(vidx[i]));
            }
            std::vector<TopoDS_Shape> lumps;
            std::function<void(const TopoDS_Shape&)> flat = [&](const TopoDS_Shape& x) {
                if (x.ShapeType() == TopAbs_COMPOUND) {
                    for (TopoDS_Iterator it(x); it.More(); it.Next()) flat(it.Value());
                } else {
                    lumps.push_back(x);
                }
            };
            flat(s->s);
            gp_GTrsf g;
            g.SetVectorialPart(gp_Mat(m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]));
            g.SetTranslationPart(gp_XYZ(m[3], m[7], m[11]));
            BRep_Builder bb;
            TopoDS_Compound out;
            bb.MakeCompound(out);
            TopTools_DataMapOfShapeShape fmap;
            int any = 0;
            for (const TopoDS_Shape& L : lumps) {
                TopTools_IndexedMapOfShape lv;
                TopExp::MapShapes(L, TopAbs_VERTEX, lv);
                int in = 0;
                for (int j = 1; j <= lv.Extent(); j++) in += moved.Contains(lv(j)) ? 1 : 0;
                if (in == 0) {
                    bb.Add(out, L);
                    continue;
                }
                if (in != lv.Extent()) {
                    set_err("only part of a piece moves");
                    return nullptr;
                }
                TopoDS_Shape nl;
                if (!similarity(m)) {
                    BRepBuilderAPI_GTransform gt(L, g, Standard_True);
                    if (!gt.IsDone()) {
                        set_err("the piece could not be scaled");
                        return nullptr;
                    }
                    nl = gt.Shape();
                    for (TopExp_Explorer x(L, TopAbs_FACE); x.More(); x.Next()) fmap.Bind(x.Current(), gt.ModifiedShape(x.Current()));
                } else {
                    gp_Trsf t;
                    t.SetValues(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]);
                    BRepBuilderAPI_Transform tr(L, t, Standard_True);
                    nl = tr.Shape();
                    for (TopExp_Explorer x(L, TopAbs_FACE); x.More(); x.Next()) fmap.Bind(x.Current(), tr.ModifiedShape(x.Current()));
                }
                bb.Add(out, nl);
                any++;
            }
            if (any == 0) {
                set_err("nothing moves");
                return nullptr;
            }
            TopoDS_Shape res = out;
            if (lumps.size() == 1) {
                TopoDS_Iterator it(out);
                res = it.Value();
            }
            TopTools_IndexedMapOfShape of, rf;
            face_map(s->s, of);
            face_map(res, rf);
            std::vector<long long> tags(rf.Extent(), -1);
            for (int i = 1; i <= of.Extent(); i++) {
                TopoDS_Shape nf = fmap.IsBound(of(i)) ? fmap(of(i)) : of(i);
                int k = rf.FindIndex(nf);
                if (k > 0 && i - 1 < (int)s->tags.size()) tags[k - 1] = s->tags[i - 1];
            }
            FShape* r = wrap(res);
            if (!r) return nullptr;
            r->tags = tags;
            return r;
        },
        (FShape*)nullptr);
}

FShape* focc_piece(const FShape* s, const int* idx, int n, const double* m, long long tag) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            BRepBuilderAPI_Sewing sew(1e-4);
            std::vector<TopoDS_Shape> copies;
            for (int i = 0; i < n; i++) {
                if (idx[i] < 0 || idx[i] >= fm.Extent()) continue;
                TopoDS_Shape c = BRepBuilderAPI_Copy(fm(idx[i] + 1)).Shape();
                copies.push_back(c);
                sew.Add(c);
            }
            if (copies.empty()) {
                set_err("no face to copy");
                return nullptr;
            }
            sew.Perform();
            TopoDS_Shape sh = sew.SewedShape();
            // its open loops closed by flat faces
            std::vector<TopoDS_Face> caps;
            ShapeAnalysis_FreeBounds fb(sh, 1e-4, Standard_False, Standard_False);
            bool closable = true;
            for (TopExp_Explorer x(fb.GetClosedWires(), TopAbs_WIRE); x.More(); x.Next()) {
                BRepBuilderAPI_MakeFace mf(TopoDS::Wire(x.Current()), Standard_True);
                if (!mf.IsDone()) {
                    closable = false;
                    break;
                }
                caps.push_back(mf.Face());
            }
            for (TopExp_Explorer x(fb.GetOpenWires(), TopAbs_WIRE); x.More(); x.Next()) closable = false;
            TopoDS_Shape out = sh;
            TopTools_MapOfShape capped;
            if (closable && !caps.empty()) {
                BRepBuilderAPI_Sewing sew2(1e-4);
                sew2.Add(sh);
                for (const TopoDS_Face& c : caps) sew2.Add(c);
                sew2.Perform();
                TopoDS_Shape closed = sew2.SewedShape();
                BRepBuilderAPI_MakeSolid ms;
                int shells = 0;
                for (TopExp_Explorer x(closed, TopAbs_SHELL); x.More(); x.Next()) {
                    ms.Add(TopoDS::Shell(x.Current()));
                    shells++;
                }
                if (shells > 0 && ms.IsDone()) {
                    ShapeFix_Solid fix;
                    fix.Init(ms.Solid());
                    fix.Perform();
                    TopoDS_Shape so = fix.Solid();
                    GProp_GProps gp;
                    BRepGProp::VolumeProperties(so, gp);
                    if (valid(so) && std::abs(gp.Mass()) > 1e-9) {
                        out = so;
                        for (const TopoDS_Face& c : caps) {
                            TopoDS_Shape im = sew2.IsModified(c) ? sew2.Modified(c) : c;
                            capped.Add(im);
                        }
                    }
                }
            }
            // the tags, in the order of the faces (kept by the move)
            std::vector<long long> tags;
            TopTools_IndexedMapOfShape om;
            face_map(out, om);
            // each copied face its own sub (1, 2…: the order of its source),
            // the caps the step's own tag (sub 0); a sewn face keeps its
            // source's surface
            for (int i = 1; i <= om.Extent(); i++) {
                if (capped.Contains(om(i))) {
                    tags.push_back(tag);
                    continue;
                }
                Handle(Geom_Surface) gs = BRep_Tool::Surface(TopoDS::Face(om(i)));
                long long sub = 1;
                for (size_t k = 0; k < copies.size(); k++) {
                    TopExp_Explorer cx(copies[k], TopAbs_FACE);
                    if (cx.More() && BRep_Tool::Surface(TopoDS::Face(cx.Current())) == gs) {
                        sub = (long long)k + 1;
                        break;
                    }
                }
                tags.push_back(tag | sub);
            }
            gp_GTrsf g;
            g.SetVectorialPart(gp_Mat(m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]));
            g.SetTranslationPart(gp_XYZ(m[3], m[7], m[11]));
            TopoDS_Shape moved;
            if (!similarity(m)) {
                BRepBuilderAPI_GTransform gt(out, g, Standard_True);
                if (!gt.IsDone()) {
                    set_err("the copy could not be scaled");
                    return nullptr;
                }
                moved = gt.Shape();
            } else {
                gp_Trsf t;
                t.SetValues(m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]);
                moved = BRepBuilderAPI_Transform(out, t, Standard_True).Shape();
            }
            FShape* r = wrap(moved);
            if (!r) return nullptr;
            TopTools_IndexedMapOfShape mm;
            face_map(moved, mm);
            r->tags = (int)tags.size() == mm.Extent() ? tags : std::vector<long long>(mm.Extent(), tag | 1);
            return r;
        },
        (FShape*)nullptr);
}

// Faces (0-based) taken away and the faces round them made longer to close
// the part again (BRepAlgoAPI_Defeaturing: a fillet's faces gone, its edge
// sharp again), the tags carried.
FShape* focc_defeature(const FShape* s, const int* idx, int n) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            BRepAlgoAPI_Defeaturing df;
            df.SetShape(s->s);
            df.SetRunParallel(Standard_False);
            int k = 0;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 0 && idx[i] < fm.Extent()) {
                    df.AddFaceToRemove(fm(idx[i] + 1));
                    k++;
                }
            if (k == 0) {
                set_err("no face to take away");
                return nullptr;
            }
            df.Build();
            if (!df.IsDone() || df.HasErrors()) {
                set_err("the faces could not be taken away");
                return nullptr;
            }
            TopoDS_Shape out = df.Shape();
            if (!valid(out)) {
                set_err("the faces could not be taken away");
                return nullptr;
            }
            FShape* r = wrap(out);
            if (!r) return nullptr;
            carry_tags(df, s, r, -1);
            return r;
        },
        (FShape*)nullptr);
}

// A solid whose shell no longer closes (faces taken out) made a sheet: its
// shells on their own beside the rest. An open solid is an invalid one,
// and every move or Delete edges after it was refused (20 nov., 11 h 48:
// "ne peut ni se supprimer ni se déplacer").
static TopoDS_Shape open_solids_as_sheets(const TopoDS_Shape& s) {
    auto open = [](const TopoDS_Shape& so) {
        for (TopExp_Explorer hx(so, TopAbs_SHELL); hx.More(); hx.Next())
            if (!BRep_Tool::IsClosed(hx.Current())) return true;
        return false;
    };
    bool any = false;
    for (TopExp_Explorer sx(s, TopAbs_SOLID); sx.More(); sx.Next())
        if (open(sx.Current())) any = true;
    if (!any) return s;
    BRep_Builder b;
    TopoDS_Compound c;
    b.MakeCompound(c);
    for (TopExp_Explorer sx(s, TopAbs_SOLID); sx.More(); sx.Next()) {
        if (!open(sx.Current())) {
            b.Add(c, sx.Current());
            continue;
        }
        for (TopExp_Explorer hx(sx.Current(), TopAbs_SHELL); hx.More(); hx.Next()) b.Add(c, hx.Current());
    }
    for (TopExp_Explorer hx(s, TopAbs_SHELL, TopAbs_SOLID); hx.More(); hx.Next()) b.Add(c, hx.Current());
    for (TopExp_Explorer fx(s, TopAbs_FACE, TopAbs_SHELL); fx.More(); fx.Next()) b.Add(c, fx.Current());
    return c;
}

FShape* focc_remove_faces(const FShape* s, const int* idx, int n) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            opencascade::handle<BRepTools_ReShape> rs = new BRepTools_ReShape();
            int k = 0;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 0 && idx[i] < fm.Extent()) {
                    rs->Remove(fm(idx[i] + 1));
                    k++;
                }
            if (k == 0) {
                set_err("no face to delete");
                return nullptr;
            }
            TopoDS_Shape out = open_solids_as_sheets(rs->Apply(s->s));
            FShape* r = wrap(out);
            if (!r) return nullptr;
            TopTools_IndexedMapOfShape fo;
            face_map(out, fo);
            r->tags.assign(fo.Extent(), -1);
            for (int j = 1; j <= fo.Extent(); j++) {
                int q = fm.FindIndex(fo(j));
                if (q > 0 && q - 1 < (int)s->tags.size()) r->tags[j - 1] = s->tags[q - 1];
            }
            return r;
        },
        (FShape*)nullptr);
}

// Delete edges (Edge mode + Del): the two faces each edge (1-based index)
// parts made one again, the edges running on through its ends joined too
// (a Knife's line taken out). Only faces lying on one surface can be one.
FShape* focc_remove_edges(const FShape* s, const int* idx, int n) {
    return guard(
        [&]() -> FShape* {
            TopTools_IndexedMapOfShape em;
            edge_map(s->s, em);
            TopTools_MapOfShape across, ends;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 1 && idx[i] <= em.Extent()) {
                    across.Add(em(idx[i]));
                    for (TopExp_Explorer vx(em(idx[i]), TopAbs_VERTEX); vx.More(); vx.Next()) ends.Add(vx.Current());
                }
            if (across.IsEmpty()) {
                set_err("no edge to delete");
                return nullptr;
            }
            FShape* res = wrap(s->s);
            if (!res) return nullptr;
            res->tags = s->tags;
            TopTools_IndexedMapOfShape before;
            edge_map(res->s, before);
            FShape* out = unify_across(res, across, &ends);
            TopTools_IndexedMapOfShape after;
            edge_map(out->s, after);
            bool gone = true;
            for (TopTools_MapIteratorOfMapOfShape it(across); it.More(); it.Next())
                if (after.Contains(it.Key())) gone = false;
            if (!gone) {
                delete out;
                set_err("its two faces are not on one surface: only a cut across a face (the Knife's) can be taken out");
                return nullptr;
            }
            return out;
        },
        (FShape*)nullptr);
}

// A tag's offset image (Shell): `base` + 1 + a hash of the tag, so each
// face's inner wall gets its own tag that stays the same from build to
// build (sub 0 is the rims').
static long long shell_inner_tag(long long base, long long t) {
    unsigned long long z = (unsigned long long)t + 0x9E3779B97F4A7C15ull;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    z ^= z >> 31;
    return base + 1 + (long long)(z % 65534ull);
}

// Whether two faces meet tangent along an edge (their normals within 2°
// at its middle).
static bool tangent_along(const TopoDS_Edge& e, const TopoDS_Face& f, const TopoDS_Face& g) {
    auto nrm = [&](const TopoDS_Face& face, gp_Dir& d) -> bool {
        double c0, c1;
        opencascade::handle<Geom2d_Curve> pc = BRep_Tool::CurveOnSurface(e, face, c0, c1);
        if (pc.IsNull()) return false;
        gp_Pnt2d uv = pc->Value((c0 + c1) / 2);
        BRepAdaptor_Surface sa(face, Standard_True);
        BRepLProp_SLProps pr(sa, uv.X(), uv.Y(), 1, 1e-7);
        if (!pr.IsNormalDefined()) return false;
        d = pr.Normal();
        if (face.Orientation() == TopAbs_REVERSED) d.Reverse();
        return true;
    };
    gp_Dir a, b;
    if (!nrm(f, a) || !nrm(g, b)) return false;
    return a.Angle(b) < 2.0 * M_PI / 180.0;
}

// The part hollowed (Shell; BRepOffsetAPI_MakeThickSolid): the faces `idx`
// (0-based) taken away as openings, the walls `t` thick inwards (out = 0)
// or outwards. Only the solids holding an opening are hollowed (every one
// when there is none: closed hollows, a cavity inside); other solids and
// sheets are kept as they are. `arc`: the gaps between the offsets of two
// faces filled round (GeomAbs_Arc) instead of the faces grown till they
// meet (GeomAbs_Intersection, sharp). A flat face to open that meets a face
// tangent to it (a fillet running into it: OpenCascade cannot end the wall
// there) is not taken away by the offset: its wall is cut through after
// (the face swept across the wall), the rounded walls round it ending
// square. Tags: a face kept its own; a face's offset
// `shell_inner_tag(base, its tag)`; the rims and the rest `base`.
FShape* focc_shell(const FShape* s, const int* idx, int n, double t, int out, long long base, int arc) {
    return guard(
        [&]() -> FShape* {
            if (!(t > 1e-6)) {
                set_err("no thickness");
                return nullptr;
            }
            TopTools_IndexedMapOfShape fm;
            face_map(s->s, fm);
            auto tag_of = [&](const TopoDS_Shape& f) -> long long {
                int i = fm.FindIndex(f);
                return i > 0 && i - 1 < (int)s->tags.size() ? s->tags[i - 1] : -1;
            };
            TopTools_MapOfShape closing;
            for (int i = 0; i < n; i++)
                if (idx[i] >= 0 && idx[i] < fm.Extent()) closing.Add(fm(idx[i] + 1));
            if (n > 0 && closing.IsEmpty()) {
                set_err("its faces are gone");
                return nullptr;
            }
            BRep_Builder bb;
            TopoDS_Compound comp;
            bb.MakeCompound(comp);
            // the tag of each face of the result, by the face itself
            std::vector<std::pair<TopoDS_Shape, long long>> given;
            int count = 0, hollowed = 0;
            for (TopExp_Explorer ex(s->s, TopAbs_SOLID); ex.More(); ex.Next()) {
                const TopoDS_Shape& so = ex.Current();
                count++;
                TopTools_IndexedDataMapOfShapeListOfShape e2f;
                TopExp::MapShapesAndAncestors(so, TopAbs_EDGE, TopAbs_FACE, e2f);
                TopTools_ListOfShape cl;
                TopTools_MapOfShape clm;
                std::vector<TopoDS_Face> cut_through;
                bool any = false;
                for (TopExp_Explorer fx(so, TopAbs_FACE); fx.More(); fx.Next()) {
                    if (!closing.Contains(fx.Current())) continue;
                    any = true;
                    TopoDS_Face f = TopoDS::Face(fx.Current());
                    bool tan = false;
                    for (TopExp_Explorer exe(f, TopAbs_EDGE); exe.More() && !tan; exe.Next()) {
                        int k = e2f.FindIndex(exe.Current());
                        if (k == 0) continue;
                        for (TopTools_ListOfShape::Iterator it(e2f(k)); it.More(); it.Next()) {
                            if (it.Value().IsSame(f) || closing.Contains(it.Value())) continue;
                            if (tangent_along(TopoDS::Edge(exe.Current()), f, TopoDS::Face(it.Value()))) tan = true;
                        }
                    }
                    if (!tan) {
                        cl.Append(f);
                        clm.Add(f);
                        continue;
                    }
                    if (BRepAdaptor_Surface(f).GetType() != GeomAbs_Plane) {
                        set_err("a curved face to open meets a face tangent to it: open that one too, or round after the Shell");
                        return nullptr;
                    }
                    cut_through.push_back(f);
                }
                if (!closing.IsEmpty() && !any) {
                    bb.Add(comp, so);
                    continue;
                }
                BRepOffsetAPI_MakeThickSolid mk;
                mk.MakeThickSolidByJoin(so, cl, out ? t : -t, 1e-5, BRepOffset_Skin, Standard_False, Standard_False, arc ? GeomAbs_Arc : GeomAbs_Intersection, Standard_True);
                if (!mk.IsDone() || mk.Shape().IsNull()) {
                    set_err(out ? "the walls cannot grow that thick out" : "the walls cannot be that thick (the part is thinner, or curved tighter, somewhere)");
                    return nullptr;
                }
                TopoDS_Shape r = mk.Shape();
                // the result is one solid (or a compound holding one)
                TopoDS_Shape one;
                int solids = 0;
                for (TopExp_Explorer sx(r, TopAbs_SOLID); sx.More(); sx.Next()) {
                    one = sx.Current();
                    solids++;
                }
                if (solids != 1) {
                    set_err(solids == 0 ? "the shell gave no solid" : "the shell came apart (the walls are too thick here)");
                    return nullptr;
                }
                if (cl.IsEmpty()) {
                    // no opening: OpenCascade gives the offset solid alone;
                    // the cavity made by hand (the inner shell reversed)
                    const TopoDS_Shape& outer = out ? one : so;
                    const TopoDS_Shape& inner = out ? so : one;
                    BRepBuilderAPI_MakeSolid ms;
                    for (TopExp_Explorer x(outer, TopAbs_SHELL); x.More(); x.Next()) ms.Add(TopoDS::Shell(x.Current()));
                    for (TopExp_Explorer x(inner, TopAbs_SHELL); x.More(); x.Next()) ms.Add(TopoDS::Shell(x.Current().Reversed()));
                    if (!ms.IsDone()) {
                        set_err("the cavity could not be made");
                        return nullptr;
                    }
                    one = ms.Solid();
                }
                if (!valid(one)) {
                    ShapeFix_Shape fix(one);
                    fix.SetPrecision(1e-6);
                    fix.SetMaxTolerance(1e-3);
                    fix.Perform();
                    if (valid(fix.Shape())) one = fix.Shape();
                }
                if (!valid(one)) {
                    dump_check(one);
                    set_err("the shell gave an invalid solid (the walls are too thick here)");
                    return nullptr;
                }
                // its faces' tags
                std::vector<std::pair<TopoDS_Shape, long long>> mine;
                for (TopExp_Explorer fx(so, TopAbs_FACE); fx.More(); fx.Next()) {
                    const TopoDS_Shape& f = fx.Current();
                    long long tg = tag_of(f);
                    if (clm.Contains(f)) {
                        const TopTools_ListOfShape& m = mk.Modified(f);
                        for (TopTools_ListOfShape::Iterator it(m); it.More(); it.Next()) mine.push_back({it.Value(), base});
                        continue;
                    }
                    mine.push_back({f, tg});
                    const TopTools_ListOfShape& m = mk.Modified(f);
                    for (TopTools_ListOfShape::Iterator it(m); it.More(); it.Next()) mine.push_back({it.Value(), tg});
                    const TopTools_ListOfShape& gn = mk.Generated(f);
                    for (TopTools_ListOfShape::Iterator it(gn); it.More(); it.Next())
                        for (TopExp_Explorer gx(it.Value(), TopAbs_FACE); gx.More(); gx.Next()) mine.push_back({gx.Current(), tg < 0 ? base : shell_inner_tag(base, tg)});
                }
                // the faces next to a tangent one: their walls cut through
                if (!cut_through.empty()) {
                    FShape cur;
                    cur.s = one;
                    TopTools_IndexedMapOfShape cm;
                    face_map(one, cm);
                    cur.tags.assign(cm.Extent(), base);
                    for (auto& [f, tg] : mine) {
                        int j = cm.FindIndex(f);
                        if (j > 0) cur.tags[j - 1] = tg;
                    }
                    for (const TopoDS_Face& f : cut_through) {
                        BRepAdaptor_Surface sa(f);
                        gp_Dir nd = sa.Plane().Axis().Direction();
                        if (f.Orientation() == TopAbs_REVERSED) nd.Reverse();
                        gp_Vec nv(nd);
                        // inwards: from just outside the face down to the
                        // wall's inner side; outwards: from just inside it
                        // to just past the wall's outer side
                        double from = out ? -0.1 : 0.1;
                        double len = out ? t + 0.2 : -(t + 0.1);
                        gp_Trsf tr;
                        tr.SetTranslation(nv * from);
                        TopoDS_Shape moved = BRepBuilderAPI_Transform(f, tr, Standard_True).Shape();
                        TopoDS_Shape prism = BRepPrimAPI_MakePrism(moved, nv * len).Shape();
                        BRepAlgoAPI_Cut cut(cur.s, prism);
                        cut.SetRunParallel(Standard_False);
                        cut.Build();
                        if (!cut.IsDone() || cut.HasErrors()) {
                            set_err("the wall by a face to open could not be cut through");
                            return nullptr;
                        }
                        FShape nxt;
                        nxt.s = cut.Shape();
                        carry_tags(cut, &cur, &nxt, base);
                        cur = nxt;
                    }
                    TopoDS_Shape only;
                    int k = 0;
                    for (TopExp_Explorer sx(cur.s, TopAbs_SOLID); sx.More(); sx.Next()) {
                        only = sx.Current();
                        k++;
                    }
                    if (k != 1 || !valid(only)) {
                        set_err(k == 1 ? "the wall by a face to open could not be cut through" : "the part came apart where its wall was cut through");
                        return nullptr;
                    }
                    one = only;
                    TopTools_IndexedMapOfShape om;
                    face_map(cur.s, om);
                    mine.clear();
                    for (int j = 1; j <= om.Extent(); j++) mine.push_back({om(j), cur.tags[j - 1]});
                }
                // a wrong solid (OpenCascade may give one with no error): a
                // wall's volume is at most its thickness times half the
                // area round it (its two sides), a little over for saddles
                GProp_GProps g0, g1, ga;
                BRepGProp::VolumeProperties(so, g0);
                BRepGProp::VolumeProperties(one, g1);
                BRepGProp::SurfaceProperties(one, ga);
                double v0 = std::fabs(g0.Mass()), v1 = g1.Mass();
                if (!(v1 > 1e-9) || (!out && v1 >= v0 * (1.0 - 1e-9)) || v1 > 0.5 * ga.Mass() * t * 1.15 + 1e-6) {
                    set_err("the shell gave a wrong solid (the walls are too thick here, or a face to open meets a rounded one)");
                    return nullptr;
                }
                bb.Add(comp, one);
                hollowed++;
                given.insert(given.end(), mine.begin(), mine.end());
            }
            if (count == 0) {
                set_err("there is no solid to hollow");
                return nullptr;
            }
            if (hollowed == 0) {
                set_err("its faces are on no solid");
                return nullptr;
            }
            // sheets beside the solids
            for (TopExp_Explorer ex(s->s, TopAbs_SHELL, TopAbs_SOLID); ex.More(); ex.Next()) bb.Add(comp, ex.Current());
            for (TopExp_Explorer ex(s->s, TopAbs_FACE, TopAbs_SHELL); ex.More(); ex.Next()) bb.Add(comp, ex.Current());
            TopoDS_Shape res = comp;
            {
                int k = 0;
                TopoDS_Shape only;
                for (TopoDS_Iterator it(comp); it.More(); it.Next()) {
                    only = it.Value();
                    k++;
                }
                if (k == 1) res = only;
            }
            FShape* r = wrap(res);
            if (!r) return nullptr;
            TopTools_IndexedMapOfShape fo;
            face_map(r->s, fo);
            r->tags.assign(fo.Extent(), -2);
            // kept faces (the solids and sheets left as they were) first
            for (int i = 1; i <= fm.Extent(); i++) {
                int j = fo.FindIndex(fm(i));
                if (j > 0) r->tags[j - 1] = tag_of(fm(i));
            }
            for (auto& [f, tg] : given) {
                int j = fo.FindIndex(f);
                if (j > 0) r->tags[j - 1] = tg;
            }
            for (long long& tg : r->tags)
                if (tg == -2) tg = base;
            return r;
        },
        (FShape*)nullptr);
}

}  // extern "C"
