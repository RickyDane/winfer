#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreMedia/CoreMedia.h>
#import <VideoToolbox/VideoToolbox.h>
#import <IOSurface/IOSurface.h>
#import <objc/runtime.h>
#import <objc/message.h>
#import <dlfcn.h>
#import <stdatomic.h>
#import <string.h>
#import <stdlib.h>

typedef void (*WinferIosFrameCb)(void *ctx, const uint8_t *data, size_t len, uint8_t is_key, uint32_t width, uint32_t height);
typedef void (*WinferIosEndCb)(void *ctx, const char *error);

typedef void *(*IndigoMouseFn)(const CGPoint *p0, const CGPoint *p1, uint32_t target, uint64_t eventType, uint32_t edge, double w, double h);
typedef void *(*IndigoButtonFn)(uint32_t keyCode, uint32_t op, uint32_t target);
typedef void *(*IndigoKeyFn)(uint32_t keyCode, uint32_t op);

// IndigoHID constants (SimulatorKit.framework).
static const uint32_t kIndigoTargetDigitizer = 0x32;
static const uint32_t kIndigoTargetHardware  = 0x33;
static const uint64_t kNSEventLeftMouseDown    = 1;
static const uint64_t kNSEventLeftMouseUp      = 2;
static const uint64_t kNSEventLeftMouseDragged = 6;
static const uint32_t kIndigoButtonDown = 1;
static const uint32_t kIndigoButtonUp   = 2;
static const uint32_t kIndigoButtonHome = 0;
static const uint32_t kIndigoButtonLock = 1;

static const uint32_t kIndigoEdgeNone   = 0;
static const uint32_t kIndigoEdgeTop    = 1;
static const uint32_t kIndigoEdgeLeft   = 2;
static const uint32_t kIndigoEdgeBottom = 3;
static const uint32_t kIndigoEdgeRight  = 4;

typedef struct {
    void *coreSim;
    void *simKit;
    IndigoMouseFn mkMouse;
    IndigoButtonFn mkButton;
    IndigoKeyFn mkKey;
    Class simServiceContextCls;
    Class hidClientCls;
    BOOL loaded;
} SimFrameworks;

static SimFrameworks g_fw = {0};
static dispatch_once_t g_fw_once;

static NSString *resolveDeveloperDir(const char *hint) {
    if (hint && hint[0] != '\0') {
        return [NSString stringWithUTF8String:hint];
    }
    const char *env = getenv("DEVELOPER_DIR");
    if (env && env[0] != '\0') {
        return [NSString stringWithUTF8String:env];
    }
    return @"/Applications/Xcode.app/Contents/Developer";
}

static void loadFrameworks(NSString *devDir) {
    dispatch_once(&g_fw_once, ^{
        g_fw.coreSim = dlopen("/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator", RTLD_NOW);
        if (!g_fw.coreSim) return;
        g_fw.simServiceContextCls = objc_getClass("SimServiceContext");
        if (!g_fw.simServiceContextCls) return;

        // SimulatorKit lives inside Xcode.app/Contents/SharedFrameworks relative to Contents/Developer.
        NSString *contentsDir = [devDir stringByDeletingLastPathComponent];
        NSArray<NSString *> *candidates = @[
            [contentsDir stringByAppendingPathComponent:@"SharedFrameworks/SimulatorKit.framework/SimulatorKit"],
            @"/Applications/Xcode.app/Contents/SharedFrameworks/SimulatorKit.framework/SimulatorKit",
            @"/Library/Developer/PrivateFrameworks/SimulatorKit.framework/SimulatorKit",
        ];
        for (NSString *path in candidates) {
            g_fw.simKit = dlopen(path.UTF8String, RTLD_NOW);
            if (g_fw.simKit) break;
        }
        if (g_fw.simKit) {
            g_fw.mkMouse = (IndigoMouseFn)dlsym(g_fw.simKit, "IndigoHIDMessageForMouseNSEvent");
            g_fw.mkButton = (IndigoButtonFn)dlsym(g_fw.simKit, "IndigoHIDMessageForButton");
            g_fw.mkKey = (IndigoKeyFn)dlsym(g_fw.simKit, "IndigoHIDMessageForKeyboardArbitrary");
            g_fw.hidClientCls = objc_getClass("_TtC12SimulatorKit24SimDeviceLegacyHIDClient");
        }
        g_fw.loaded = YES;
    });
}

static void writeError(char *buf, size_t cap, NSString *msg) {
    if (!buf || cap == 0) return;
    const char *s = msg ? msg.UTF8String : "unknown error";
    strlcpy(buf, s, cap);
}

static uint32_t evenScaled(size_t side, double scale) {
    uint32_t v = (uint32_t)(side * scale);
    v = (v / 2) * 2;
    return v < 2 ? 2 : v;
}

@interface WinferIosSimSession : NSObject {
@public
    id _device;
    id _displayDesc;
    id _hidClient;
    NSUUID *_damageUUID;
    NSUUID *_surfaceUUID;

    dispatch_queue_t _encodeQueue;
    dispatch_queue_t _hidQueue;
    VTCompressionSessionRef _vt;

    uint32_t _maxSide;
    uint32_t _maxFps;
    uint32_t _bitRate;
    uint32_t _curW;
    uint32_t _curH;
    int64_t _pts;
    uint64_t _lastEncodeNs;

    atomic_bool _stopped;
    atomic_bool _pending;
    atomic_bool _timerArmed;
    atomic_bool _forceKey;

    BOOL _touchDown;
    uint32_t _activeEdge;

    WinferIosFrameCb _onFrame;
    WinferIosEndCb _onEnd;
    void *_ctx;
}
@end

@implementation WinferIosSimSession
@end

static void vtOutputCallback(
    void *outputCallbackRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTEncodeInfoFlags infoFlags,
    CMSampleBufferRef sb
) {
    (void)sourceFrameRefCon;
    (void)infoFlags;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)outputCallbackRefCon;
    if (!s || atomic_load(&s->_stopped)) return;
    if (status != noErr || !sb || !CMSampleBufferDataIsReady(sb)) return;

    CMBlockBufferRef dataBuf = CMSampleBufferGetDataBuffer(sb);
    if (!dataBuf) return;

    size_t totalLen = CMBlockBufferGetDataLength(dataBuf);
    if (totalLen == 0) return;

    BOOL isKey = YES;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sb, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        CFDictionaryRef dict = CFArrayGetValueAtIndex(attachments, 0);
        CFBooleanRef notSync = CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync);
        if (notSync == kCFBooleanTrue) isKey = NO;
    }

    // Reserve enough for SPS/PPS + Annex-B NALUs (4-byte start code replaces 4-byte AVCC length).
    size_t cap = totalLen + 256;
    uint8_t *out = (uint8_t *)malloc(cap);
    if (!out) return;
    size_t outLen = 0;
    static const uint8_t startCode[4] = {0, 0, 0, 1};

    if (isKey) {
        CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sb);
        if (fmt) {
            size_t psCount = 0;
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, 0, NULL, NULL, &psCount, NULL);
            for (size_t i = 0; i < psCount; i++) {
                const uint8_t *ps = NULL;
                size_t psLen = 0;
                if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, i, &ps, &psLen, NULL, NULL) == noErr && ps && psLen > 0) {
                    if (outLen + 4 + psLen > cap) {
                        cap = (outLen + 4 + psLen) * 2;
                        uint8_t *grown = (uint8_t *)realloc(out, cap);
                        if (!grown) { free(out); return; }
                        out = grown;
                    }
                    memcpy(out + outLen, startCode, 4);
                    outLen += 4;
                    memcpy(out + outLen, ps, psLen);
                    outLen += psLen;
                }
            }
        }
    }

    char *contiguousPtr = NULL;
    uint8_t *copiedBuf = NULL;
    const uint8_t *src = NULL;
    if (CMBlockBufferIsRangeContiguous(dataBuf, 0, totalLen)) {
        size_t lenAtOffset = 0;
        if (CMBlockBufferGetDataPointer(dataBuf, 0, &lenAtOffset, NULL, &contiguousPtr) == kCMBlockBufferNoErr && lenAtOffset >= totalLen) {
            src = (const uint8_t *)contiguousPtr;
        }
    }
    if (!src) {
        copiedBuf = (uint8_t *)malloc(totalLen);
        if (!copiedBuf) { free(out); return; }
        if (CMBlockBufferCopyDataBytes(dataBuf, 0, totalLen, copiedBuf) != kCMBlockBufferNoErr) {
            free(copiedBuf);
            free(out);
            return;
        }
        src = copiedBuf;
    }

    size_t offset = 0;
    while (offset + 4 <= totalLen) {
        uint32_t naluLen = 0;
        memcpy(&naluLen, src + offset, 4);
        naluLen = CFSwapInt32BigToHost(naluLen);
        offset += 4;
        if (naluLen == 0 || offset + naluLen > totalLen) break;
        if (outLen + 4 + naluLen > cap) {
            cap = (outLen + 4 + naluLen) * 2;
            uint8_t *grown = (uint8_t *)realloc(out, cap);
            if (!grown) { free(copiedBuf); free(out); return; }
            out = grown;
        }
        memcpy(out + outLen, startCode, 4);
        outLen += 4;
        memcpy(out + outLen, src + offset, naluLen);
        outLen += naluLen;
        offset += naluLen;
    }

    free(copiedBuf);

    if (outLen > 0 && !atomic_load(&s->_stopped) && s->_onFrame) {
        s->_onFrame(s->_ctx, out, outLen, isKey ? 1 : 0, s->_curW, s->_curH);
    }
    free(out);
}

static void applyEncoderRate(VTCompressionSessionRef vt, uint32_t bitRate, uint32_t maxFps) {
    if (!vt) return;
    int32_t br = (int32_t)bitRate;
    CFNumberRef brNum = CFNumberCreate(NULL, kCFNumberSInt32Type, &br);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_AverageBitRate, brNum);
    CFRelease(brNum);

    // Cap burst rate at 1.5x average bitrate over 1 second so LAN/Relay never spikes.
    int32_t bytesPerSec = (int32_t)((bitRate / 8) * 3 / 2);
    if (bytesPerSec < 128 * 1024) bytesPerSec = 128 * 1024;
    int32_t oneSec = 1;
    CFNumberRef limBytes = CFNumberCreate(NULL, kCFNumberSInt32Type, &bytesPerSec);
    CFNumberRef limSec = CFNumberCreate(NULL, kCFNumberSInt32Type, &oneSec);
    const void *vals[2] = {limBytes, limSec};
    CFArrayRef limits = CFArrayCreate(NULL, vals, 2, &kCFTypeArrayCallBacks);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_DataRateLimits, limits);
    CFRelease(limits);
    CFRelease(limBytes);
    CFRelease(limSec);

    int32_t fps = (int32_t)(maxFps > 0 ? maxFps : 60);
    CFNumberRef fpsNum = CFNumberCreate(NULL, kCFNumberSInt32Type, &fps);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_ExpectedFrameRate, fpsNum);
    CFRelease(fpsNum);

    int32_t keyInterval = fps * 2;
    CFNumberRef kiNum = CFNumberCreate(NULL, kCFNumberSInt32Type, &keyInterval);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameInterval, kiNum);
    CFRelease(kiNum);
}

static BOOL ensureEncoder(WinferIosSimSession *s, uint32_t vw, uint32_t vh) {
    if (s->_vt && s->_curW == vw && s->_curH == vh) {
        return YES;
    }
    if (s->_vt) {
        VTCompressionSessionInvalidate(s->_vt);
        CFRelease(s->_vt);
        s->_vt = NULL;
    }
    VTCompressionSessionRef vt = NULL;
    OSStatus st = VTCompressionSessionCreate(
        NULL,
        (int32_t)vw,
        (int32_t)vh,
        kCMVideoCodecType_H264,
        NULL,
        NULL,
        NULL,
        vtOutputCallback,
        (__bridge void *)s,
        &vt
    );
    if (st != noErr || !vt) {
        return NO;
    }
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    applyEncoderRate(vt, s->_bitRate, s->_maxFps);
    VTCompressionSessionPrepareToEncodeFrames(vt);
    s->_vt = vt;
    s->_curW = vw;
    s->_curH = vh;
    atomic_store(&s->_forceKey, true);
    return YES;
}

// Must run on s->_encodeQueue.
static void encodeNowOnQueue(WinferIosSimSession *s) {
    if (atomic_load(&s->_stopped)) return;
    atomic_store(&s->_pending, false);

    id rawSurf = ((id(*)(id, SEL))objc_msgSend)(s->_displayDesc, sel_registerName("framebufferSurface"));
    if (!rawSurf) return;
    IOSurfaceRef surf = (__bridge IOSurfaceRef)rawSurf;
    size_t sw = IOSurfaceGetWidth(surf);
    size_t sh = IOSurfaceGetHeight(surf);
    if (sw == 0 || sh == 0) return;

    double scale = (double)s->_maxSide / (double)(sw > sh ? sw : sh);
    if (scale > 1.0) scale = 1.0;
    uint32_t vw = evenScaled(sw, scale);
    uint32_t vh = evenScaled(sh, scale);

    if (!ensureEncoder(s, vw, vh)) return;

    CVPixelBufferRef pb = NULL;
    if (CVPixelBufferCreateWithIOSurface(NULL, surf, NULL, &pb) != kCVReturnSuccess || !pb) return;

    BOOL forceKey = atomic_exchange(&s->_forceKey, false);
    NSDictionary *props = forceKey ? @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES} : nil;
    uint64_t nowNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
    s->_lastEncodeNs = nowNs;

    CMTime pts = CMTimeMake(s->_pts++, 60);
    VTCompressionSessionEncodeFrame(s->_vt, pb, pts, kCMTimeInvalid, (__bridge CFDictionaryRef)props, NULL, NULL);
    CVPixelBufferRelease(pb);
}

static void scheduleEncode(WinferIosSimSession *s) {
    if (atomic_load(&s->_stopped)) return;
    // Coalesce burst damage callbacks: if one is already pending, nothing more to enqueue.
    if (atomic_exchange(&s->_pending, true)) return;

    dispatch_async(s->_encodeQueue, ^{
        if (atomic_load(&s->_stopped)) return;
        uint32_t fps = s->_maxFps > 0 ? s->_maxFps : 60;
        uint64_t minIntervalNs = 1000000000ULL / (uint64_t)fps;
        uint64_t nowNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
        uint64_t elapsedNs = nowNs >= s->_lastEncodeNs ? (nowNs - s->_lastEncodeNs) : minIntervalNs;

        if (elapsedNs >= minIntervalNs) {
            encodeNowOnQueue(s);
        } else if (!atomic_exchange(&s->_timerArmed, true)) {
            uint64_t waitNs = minIntervalNs - elapsedNs;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)waitNs), s->_encodeQueue, ^{
                atomic_store(&s->_timerArmed, false);
                if (!atomic_load(&s->_stopped) && atomic_load(&s->_pending)) {
                    encodeNowOnQueue(s);
                }
            });
        }
    });
}

static void sendHidMessage(WinferIosSimSession *s, void *msg) {
    if (!msg || !s->_hidClient) {
        if (msg) free(msg);
        return;
    }
    ((void(*)(id, SEL, void *, BOOL, id, id))objc_msgSend)(
        s->_hidClient,
        sel_registerName("sendWithMessage:freeWhenDone:completionQueue:completion:"),
        msg,
        YES,
        nil,
        nil
    );
}

int winfer_ios_sim_native_available(const char *developer_dir) {
    @autoreleasepool {
        loadFrameworks(resolveDeveloperDir(developer_dir));
        return (g_fw.loaded && g_fw.simKit && g_fw.mkMouse && g_fw.hidClientCls) ? 1 : 0;
    }
}

void *winfer_ios_sim_start(
    const char *developer_dir,
    const char *udid_cstr,
    uint32_t max_side,
    uint32_t max_fps,
    uint32_t bit_rate,
    WinferIosFrameCb on_frame,
    WinferIosEndCb on_end,
    void *ctx,
    uint32_t *out_width,
    uint32_t *out_height,
    uint8_t *out_has_hid,
    char *err_buf,
    size_t err_cap
) {
    @autoreleasepool {
        NSString *devDir = resolveDeveloperDir(developer_dir);
        loadFrameworks(devDir);
        if (!g_fw.loaded || !g_fw.simServiceContextCls) {
            writeError(err_buf, err_cap, @"CoreSimulator.framework unavailable");
            return NULL;
        }
        if (!udid_cstr) {
            writeError(err_buf, err_cap, @"missing simulator udid");
            return NULL;
        }
        NSUUID *udid = [[NSUUID alloc] initWithUUIDString:[NSString stringWithUTF8String:udid_cstr]];
        if (!udid) {
            writeError(err_buf, err_cap, @"invalid simulator udid");
            return NULL;
        }

        NSError *err = nil;
        id serviceCtx = ((id(*)(id, SEL, id, NSError **))objc_msgSend)(
            g_fw.simServiceContextCls,
            sel_registerName("sharedServiceContextForDeveloperDir:error:"),
            devDir,
            &err
        );
        if (!serviceCtx) {
            writeError(err_buf, err_cap, err.localizedDescription ?: @"failed to open SimServiceContext");
            return NULL;
        }
        id deviceSet = ((id(*)(id, SEL, NSError **))objc_msgSend)(
            serviceCtx,
            sel_registerName("defaultDeviceSetWithError:"),
            &err
        );
        if (!deviceSet) {
            writeError(err_buf, err_cap, err.localizedDescription ?: @"failed to open SimDeviceSet");
            return NULL;
        }
        NSDictionary *devices = ((id(*)(id, SEL))objc_msgSend)(deviceSet, sel_registerName("devicesByUDID"));
        id device = devices[udid];
        if (!device) {
            writeError(err_buf, err_cap, @"simulator not found");
            return NULL;
        }

        id io = ((id(*)(id, SEL))objc_msgSend)(device, sel_registerName("io"));
        if (!io) {
            writeError(err_buf, err_cap, @"simulator IO unavailable (is it booted?)");
            return NULL;
        }

        id displayDesc = nil;
        NSArray *ports = ((id(*)(id, SEL))objc_msgSend)(io, sel_registerName("ioPorts"));
        for (id port in ports) {
            uint16_t pcls = ((uint16_t(*)(id, SEL))objc_msgSend)(port, sel_registerName("ioPortClass"));
            if (pcls != 1) continue;
            id desc = ((id(*)(id, SEL))objc_msgSend)(port, sel_registerName("descriptor"));
            if (!desc) continue;
            id stObj = [desc respondsToSelector:sel_registerName("state")]
                ? ((id(*)(id, SEL))objc_msgSend)(desc, sel_registerName("state"))
                : nil;
            uint16_t dclass = 99;
            if (stObj && [stObj respondsToSelector:sel_registerName("displayClass")]) {
                dclass = ((uint16_t(*)(id, SEL))objc_msgSend)(stObj, sel_registerName("displayClass"));
            }
            if (dclass == 0) {
                displayDesc = desc;
                break;
            }
            if (!displayDesc) {
                displayDesc = desc;
            }
        }
        if (!displayDesc) {
            writeError(err_buf, err_cap, @"simulator display port not ready");
            return NULL;
        }

        // Wait briefly if the simulator just finished booting and IOSurface is still initializing.
        id rawSurf = ((id(*)(id, SEL))objc_msgSend)(displayDesc, sel_registerName("framebufferSurface"));
        for (int i = 0; i < 15 && !rawSurf; i++) {
            usleep(100000);
            rawSurf = ((id(*)(id, SEL))objc_msgSend)(displayDesc, sel_registerName("framebufferSurface"));
        }
        if (!rawSurf) {
            writeError(err_buf, err_cap, @"simulator framebuffer IOSurface unavailable");
            return NULL;
        }

        IOSurfaceRef surf = (__bridge IOSurfaceRef)rawSurf;
        size_t sw = IOSurfaceGetWidth(surf);
        size_t sh = IOSurfaceGetHeight(surf);
        if (sw == 0 || sh == 0) {
            writeError(err_buf, err_cap, @"simulator framebuffer size is zero");
            return NULL;
        }

        uint32_t side = max_side > 0 ? max_side : 1280;
        double scale = (double)side / (double)(sw > sh ? sw : sh);
        if (scale > 1.0) scale = 1.0;
        uint32_t vw = evenScaled(sw, scale);
        uint32_t vh = evenScaled(sh, scale);

        id hidClient = nil;
        if (g_fw.hidClientCls && g_fw.mkMouse) {
            NSError *hidErr = nil;
            for (int i = 0; i < 5 && !hidClient; i++) {
                if (i > 0) usleep(100000);
                hidErr = nil;
                hidClient = ((id(*)(id, SEL, id, NSError **))objc_msgSend)(
                    [g_fw.hidClientCls alloc],
                    sel_registerName("initWithDevice:error:"),
                    device,
                    &hidErr
                );
            }
            if (!hidClient && hidErr) {
                fprintf(stderr, "ios_sim: SimDeviceLegacyHIDClient init failed: %s\n", hidErr.localizedDescription.UTF8String);
            }
        }

        WinferIosSimSession *s = [[WinferIosSimSession alloc] init];
        s->_device = device;
        s->_displayDesc = displayDesc;
        s->_hidClient = hidClient;
        s->_encodeQueue = dispatch_queue_create("com.winfer.ios-sim.encode", DISPATCH_QUEUE_SERIAL);
        s->_hidQueue = dispatch_queue_create("com.winfer.ios-sim.hid", DISPATCH_QUEUE_SERIAL);
        s->_maxSide = side;
        s->_maxFps = max_fps > 0 ? max_fps : 60;
        s->_bitRate = bit_rate > 0 ? bit_rate : 6000000;
        s->_onFrame = on_frame;
        s->_onEnd = on_end;
        s->_ctx = ctx;
        atomic_init(&s->_stopped, false);
        atomic_init(&s->_pending, false);
        atomic_init(&s->_timerArmed, false);
        atomic_init(&s->_forceKey, true);

        __block BOOL encoderOk = NO;
        dispatch_sync(s->_encodeQueue, ^{
            encoderOk = ensureEncoder(s, vw, vh);
        });
        if (!encoderOk) {
            writeError(err_buf, err_cap, @"VideoToolbox H.264 encoder failed to start");
            return NULL;
        }

        __weak WinferIosSimSession *weakSelf = s;
        s->_damageUUID = [NSUUID UUID];
        ((void(*)(id, SEL, id, id))objc_msgSend)(
            displayDesc,
            sel_registerName("registerCallbackWithUUID:damageRectanglesCallback:"),
            s->_damageUUID,
            ^(NSArray *rects) {
                // CoreSimulator passes an empty NSArray (__NSArray0) to signal full-surface
                // frame updates; never skip empty arrays or the live stream stays frozen.
                (void)rects;
                WinferIosSimSession *strong = weakSelf;
                if (strong) scheduleEncode(strong);
            }
        );

        s->_surfaceUUID = [NSUUID UUID];
        ((void(*)(id, SEL, id, id))objc_msgSend)(
            displayDesc,
            sel_registerName("registerCallbackWithUUID:ioSurfacesChangeCallback:"),
            s->_surfaceUUID,
            ^(NSDictionary *info) {
                (void)info;
                WinferIosSimSession *strong = weakSelf;
                if (strong) {
                    atomic_store(&strong->_forceKey, true);
                    scheduleEncode(strong);
                }
            }
        );

        // Emit an initial keyframe immediately so the viewer displays the screen without waiting for movement.
        dispatch_async(s->_encodeQueue, ^{
            encodeNowOnQueue(s);
        });

        if (out_width) *out_width = vw;
        if (out_height) *out_height = vh;
        if (out_has_hid) *out_has_hid = (hidClient != nil) ? 1 : 0;

        return (void *)CFBridgingRetain(s);
    }
}

void winfer_ios_sim_set_profile(void *handle, uint32_t max_fps, uint32_t bit_rate) {
    if (!handle) return;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)handle;
    if (atomic_load(&s->_stopped)) return;
    dispatch_async(s->_encodeQueue, ^{
        if (atomic_load(&s->_stopped)) return;
        if (max_fps > 0) s->_maxFps = max_fps;
        if (bit_rate > 0) s->_bitRate = bit_rate;
        if (s->_vt) {
            applyEncoderRate(s->_vt, s->_bitRate, s->_maxFps);
        }
    });
}

void winfer_ios_sim_force_keyframe(void *handle) {
    if (!handle) return;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)handle;
    if (atomic_load(&s->_stopped)) return;
    atomic_store(&s->_forceKey, true);
    scheduleEncode(s);
}

static uint32_t edgeForPoint(double x, double y) {
    if (y < 0.035) return kIndigoEdgeTop;
    if (y > 0.965) return kIndigoEdgeBottom;
    if (x < 0.03) return kIndigoEdgeLeft;
    if (x > 0.97) return kIndigoEdgeRight;
    return kIndigoEdgeNone;
}

int winfer_ios_sim_touch(void *handle, uint8_t action, double x, double y) {
    if (!handle) return 0;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)handle;
    if (atomic_load(&s->_stopped) || !s->_hidClient || !g_fw.mkMouse) return 0;

    if (x < 0.0) x = 0.0;
    if (x > 1.0) x = 1.0;
    if (y < 0.0) y = 0.0;
    if (y > 1.0) y = 1.0;

    dispatch_async(s->_hidQueue, ^{
        if (atomic_load(&s->_stopped)) return;
        CGPoint pt = CGPointMake(x, y);
        uint64_t evType = kNSEventLeftMouseDragged;
        uint32_t edge = s->_activeEdge;
        if (action == 0) {
            edge = edgeForPoint(x, y);
            s->_activeEdge = edge;
            s->_touchDown = YES;
            evType = kNSEventLeftMouseDown;
        } else if (action == 2) {
            evType = kNSEventLeftMouseUp;
            s->_touchDown = NO;
            s->_activeEdge = kIndigoEdgeNone;
        } else {
            if (!s->_touchDown) return;
            evType = kNSEventLeftMouseDragged;
        }
        void *msg = g_fw.mkMouse(&pt, NULL, kIndigoTargetDigitizer, evType, edge, 1.0, 1.0);
        if (msg) {
            sendHidMessage(s, msg);
        }
    });
    return 1;
}

int winfer_ios_sim_button(void *handle, uint8_t button) {
    if (!handle) return 0;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)handle;
    if (atomic_load(&s->_stopped) || !s->_hidClient || !g_fw.mkButton) return 0;

    uint32_t src = (button == 1) ? kIndigoButtonLock : kIndigoButtonHome;
    dispatch_async(s->_hidQueue, ^{
        if (atomic_load(&s->_stopped)) return;
        sendHidMessage(s, g_fw.mkButton(src, kIndigoButtonDown, kIndigoTargetHardware));
        usleep(25000);
        if (atomic_load(&s->_stopped)) return;
        sendHidMessage(s, g_fw.mkButton(src, kIndigoButtonUp, kIndigoTargetHardware));
    });
    return 1;
}

int winfer_ios_sim_key(void *handle, uint32_t usage, uint8_t action) {
    if (!handle) return 0;
    WinferIosSimSession *s = (__bridge WinferIosSimSession *)handle;
    if (atomic_load(&s->_stopped) || !s->_hidClient || !g_fw.mkKey) return 0;

    dispatch_async(s->_hidQueue, ^{
        if (atomic_load(&s->_stopped)) return;
        if (action == 0) {
            sendHidMessage(s, g_fw.mkKey(usage, kIndigoButtonDown));
        } else if (action == 1) {
            sendHidMessage(s, g_fw.mkKey(usage, kIndigoButtonUp));
        } else {
            sendHidMessage(s, g_fw.mkKey(usage, kIndigoButtonDown));
            usleep(6000);
            if (!atomic_load(&s->_stopped)) {
                sendHidMessage(s, g_fw.mkKey(usage, kIndigoButtonUp));
            }
        }
    });
    return 1;
}

void winfer_ios_sim_stop(void *handle) {
    if (!handle) return;
    @autoreleasepool {
        WinferIosSimSession *s = (WinferIosSimSession *)CFBridgingRelease(handle);
        atomic_store(&s->_stopped, true);

        if (s->_displayDesc && s->_damageUUID) {
            ((void(*)(id, SEL, id))objc_msgSend)(
                s->_displayDesc,
                sel_registerName("unregisterDamageRectanglesCallbackWithUUID:"),
                s->_damageUUID
            );
        }
        if (s->_displayDesc && s->_surfaceUUID) {
            ((void(*)(id, SEL, id))objc_msgSend)(
                s->_displayDesc,
                sel_registerName("unregisterIOSurfacesChangeCallbackWithUUID:"),
                s->_surfaceUUID
            );
        }

        // Drain encodeQueue and invalidate VTCompressionSession so no further callbacks fire.
        dispatch_sync(s->_encodeQueue, ^{
            if (s->_vt) {
                VTCompressionSessionInvalidate(s->_vt);
                CFRelease(s->_vt);
                s->_vt = NULL;
            }
        });
        dispatch_sync(s->_hidQueue, ^{
            s->_hidClient = nil;
        });

        s->_onFrame = NULL;
        s->_onEnd = NULL;
        s->_ctx = NULL;
    }
}
