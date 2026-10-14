#if defined(CONTROL_USE_A)
#include "a.h"
#else
#include "b.h"
#endif
struct CONTROL_REQUIRED { CONTROL_ENUM value; };
