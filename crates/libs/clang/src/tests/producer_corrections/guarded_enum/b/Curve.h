#ifndef AUDIO_CURVE_FADE
#define AUDIO_CURVE_FADE() 1
#endif
#ifndef _AUDIO_CURVE_TYPE_
#define _AUDIO_CURVE_TYPE_
typedef enum {
    AUDIO_CURVE_TYPE_NONE = 0,
    AUDIO_CURVE_TYPE_WINDOWS_FADE = AUDIO_CURVE_FADE()
} AUDIO_CURVE_TYPE;
#endif
struct B_ROOT {
    __attribute__((annotate("win32metadata:associated_enum=AUDIO_CURVE_TYPE")))
    unsigned value;
};
