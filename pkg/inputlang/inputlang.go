//go:build darwin && cgo

package inputlang

/*
#cgo LDFLAGS: -framework Carbon -framework CoreFoundation
#include <Carbon/Carbon.h>
#include <stdlib.h>

static char* get_current_input_lang() {
    TISInputSourceRef source = TISCopyCurrentKeyboardInputSource();
    if (!source) return NULL;

    CFArrayRef langs = (CFArrayRef)TISGetInputSourceProperty(
        source, kTISPropertyInputSourceLanguages);

    char* result = NULL;
    if (langs && CFArrayGetCount(langs) > 0) {
        CFStringRef lang = (CFStringRef)CFArrayGetValueAtIndex(langs, 0);
        if (lang) {
            CFIndex maxLen = CFStringGetMaximumSizeForEncoding(
                CFStringGetLength(lang), kCFStringEncodingUTF8) + 1;
            result = (char*)malloc(maxLen);
            if (!CFStringGetCString(lang, result, maxLen, kCFStringEncodingUTF8)) {
                free(result);
                result = NULL;
            }
        }
    }

    CFRelease(source);
    return result;
}
*/
import "C"
import "unsafe"

// Current returns the BCP-47 language code of the active macOS keyboard input source.
// Returns "" if the language cannot be determined.
func Current() string {
	cstr := C.get_current_input_lang()
	if cstr == nil {
		return ""
	}
	defer C.free(unsafe.Pointer(cstr))
	return C.GoString(cstr)
}
