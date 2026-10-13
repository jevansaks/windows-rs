#if defined(AUTHORITY_OBSERVER_CLIENT) && defined(AUTHORITY_OBSERVER_FORWARD_ONLY)
struct ObserverRecord;
struct ObserverRecord;
#else
#if defined(AUTHORITY_OBSERVER_CLIENT) && defined(AUTHORITY_OBSERVER_FORWARD_DEFINED)
struct ObserverRecord;
struct ObserverRecord;
#endif
struct ObserverRecord {
#if defined(AUTHORITY_OBSERVER_CLIENT) && !defined(AUTHORITY_OBSERVER_EQUAL)
    unsigned long long value;
#else
    int value;
#endif
};
#endif
