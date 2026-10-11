#pragma once
#ifndef BUFFER_SILENT_VALUE
#define BUFFER_SILENT_VALUE 2
#endif
typedef enum BufferFlags {
    BUFFER_INVALID = 0,
    BUFFER_VALID = 1,
    BUFFER_SILENT = BUFFER_SILENT_VALUE
} BufferFlags;
