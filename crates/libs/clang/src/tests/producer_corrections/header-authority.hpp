typedef AUTHORITY_SCALAR AUTHORITY_ALIAS;
struct AUTHORITY_RECORD { AUTHORITY_SCALAR value; };
const int AUTHORITY_CONSTANT = AUTHORITY_VALUE;

#ifdef AUTHORITY_INSPECTION
extern "C" __attribute__((annotate(AUTHORITY_IMPORT)))
void InspectAuthority(AUTHORITY_RECORD* value);
#ifdef AUTHORITY_ALTERNATIVE
extern "C" __attribute__((annotate("win32metadata:import_library=losing.dll")))
void AlternativeOnly();
#endif
#endif
