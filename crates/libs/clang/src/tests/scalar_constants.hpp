namespace Scope { const int VALUE = REPORT_VALUE; }
const int EQUAL = 7;
const REPORT_TYPE TYPED = 7;
const float FLOAT = REPORT_VALUE;
#ifdef REPORT_ALTERNATIVE
namespace B { const int SCOPED = REPORT_VALUE; }
namespace A { struct C { static const int OUT; }; }
const int A::C::OUT = 7;
#else
namespace A { const int SCOPED = REPORT_VALUE; }
namespace A { struct C { static const int OUT = 7; }; }
#endif
enum { FIRST = REPORT_VALUE, SECOND = 11 };
enum Named { PROJECTED_MEMBER = REPORT_VALUE };
#define MACRO REPORT_VALUE
extern const int ABSENT;
const int* const POINTER = nullptr;
extern "C" int UseScalar(int value);
#ifdef REPORT_SCOPING_ANCHORS
struct RENAMED { int shared_member; };
#endif
