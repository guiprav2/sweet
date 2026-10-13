#include "quickjs.h"

#include <stdio.h>
#include <string.h>

#define countof(x) (sizeof(x) / sizeof((x)[0]))

#define SWEET_CLICK 1
#define SWEET_MOUSEDOWN 2
#define SWEET_MOUSEUP 3
#define SWEET_MOUSEMOVE 4
#define SWEET_INPUT 5
#define SWEET_KEYDOWN 7
#define SWEET_CLOSE 8

typedef struct SweetDoc SweetDoc;

typedef struct {
    uint64_t token;
    uint64_t doc_id;
    double x;
    double y;
    uint32_t kind;
    uint32_t has_key;
    int32_t target;
    int32_t listen_on;
    char key[64];
} SweetEvent;

typedef struct {
    uint32_t id;
    int32_t parent;
    double x;
    double y;
    double width;
    double height;
} SweetBox;

extern SweetDoc *sweet_doc_new(char *err, size_t err_len);
extern SweetDoc *sweet_doc_clone(SweetDoc *doc);
extern void sweet_doc_free(SweetDoc *doc);
extern uint64_t sweet_doc_id(SweetDoc *doc);
extern int sweet_doc_same(SweetDoc *a, SweetDoc *b);
extern void sweet_string_free(char *ptr);
extern void sweet_bytes_free(uint8_t *ptr, size_t len);
extern void sweet_u32_free(uint32_t *ptr, size_t len);
extern void sweet_box_free(SweetBox *ptr, size_t len);
extern uint32_t sweet_body(SweetDoc *doc);
extern int sweet_dark(SweetDoc *doc);
extern void sweet_set_dark(SweetDoc *doc, int dark);
extern char *sweet_tag_name(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern char *sweet_class_name(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern char *sweet_text_content(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern char *sweet_id_attr(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern char *sweet_control_value(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern int sweet_node_type(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern int sweet_set_class_name(SweetDoc *doc, uint32_t id, const char *value, char *err, size_t err_len);
extern int sweet_set_text_content(SweetDoc *doc, uint32_t id, const char *value, char *err, size_t err_len);
extern int sweet_set_id_attr(SweetDoc *doc, uint32_t id, const char *value, char *err, size_t err_len);
extern int sweet_set_control_value(SweetDoc *doc, uint32_t id, const char *value, char *err, size_t err_len);
extern int sweet_disabled(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern int sweet_set_disabled(SweetDoc *doc, uint32_t id, int disabled, char *err, size_t err_len);
extern int sweet_create_element(SweetDoc *doc, const char *tag, uint32_t *out_id, char *err, size_t err_len);
extern int sweet_create_text(SweetDoc *doc, const char *text, uint32_t *out_id, char *err, size_t err_len);
extern int sweet_create_comment(SweetDoc *doc, const char *text, uint32_t *out_id, char *err, size_t err_len);
extern int sweet_set_attribute(SweetDoc *doc, uint32_t id, const char *name, const char *value, char *err, size_t err_len);
extern int sweet_get_attribute(SweetDoc *doc, uint32_t id, const char *name, char **out, char *err, size_t err_len);
extern int sweet_remove_attribute(SweetDoc *doc, uint32_t id, const char *name, char *err, size_t err_len);
extern int sweet_set_image(SweetDoc *doc, uint32_t id, const uint8_t *bytes, size_t len, uint32_t *out_w, uint32_t *out_h, char *err, size_t err_len);
extern int sweet_clear_image(SweetDoc *doc, uint32_t id, char *err, size_t err_len);
extern int sweet_insert_before(SweetDoc *doc, uint32_t parent, uint32_t child, int has_before, uint32_t before, char *err, size_t err_len);
extern int sweet_remove_child(SweetDoc *doc, uint32_t parent, uint32_t child, char *err, size_t err_len);
extern int sweet_children(SweetDoc *doc, uint32_t id, uint32_t **out_ptr, size_t *out_len, char *err, size_t err_len);
extern int sweet_parent(SweetDoc *doc, uint32_t id, uint32_t *out_id, char *err, size_t err_len);
extern int sweet_render_png(SweetDoc *doc, uint32_t width, uint32_t height, uint8_t **out_ptr, size_t *out_len, char *err, size_t err_len);
extern int sweet_layout_boxes(SweetDoc *doc, double width, double height, SweetBox **out_ptr, size_t *out_len, char *err, size_t err_len);
extern void sweet_listen(SweetDoc *doc, const char *event);
extern int sweet_window_open(SweetDoc *doc, const char *title, double width, double height, uint64_t *out_token, char *err, size_t err_len);
extern void sweet_window_close(uint64_t token);
extern int sweet_windows_alive(void);
extern int sweet_notify_fd(void);
extern void sweet_kick(void);
extern int sweet_poll(SweetEvent *out);

static JSClassID doc_class_id;
static JSClassID node_class_id;
static JSClassID win_class_id;
static JSClassID cleanup_class_id;

typedef struct {
    SweetDoc *doc;
} DocOpaque;

typedef struct {
    SweetDoc *doc;
    uint32_t id;
} NodeOpaque;

typedef struct {
    uint64_t token;
    SweetDoc *doc;
} WinOpaque;

typedef struct Listener {
    uint64_t doc_id;
    int32_t node;
    char event[32];
    JSValue func;
    struct Listener *next;
} Listener;

typedef struct CloseWait {
    uint64_t token;
    JSValue resolve;
    struct CloseWait *next;
} CloseWait;

static Listener *listeners;
static CloseWait *waits;

static JSValue throw_err(JSContext *ctx, const char *err)
{
    if (err && err[0])
        return JS_ThrowTypeError(ctx, "%s", err);
    return JS_ThrowTypeError(ctx, "sweet error");
}

static JSValue take_cstr(JSContext *ctx, char *text, const char *err)
{
    JSValue value;
    if (!text)
        return throw_err(ctx, err);
    value = JS_NewString(ctx, text);
    sweet_string_free(text);
    return value;
}

static int read_cstr(JSContext *ctx, JSValueConst value, const char **out)
{
    const char *text = JS_ToCString(ctx, value);
    if (!text)
        return -1;
    *out = text;
    return 0;
}

static JSValue new_node(JSContext *ctx, SweetDoc *doc, uint32_t id)
{
    NodeOpaque *node;
    JSValue obj = JS_NewObjectClass(ctx, node_class_id);
    if (JS_IsException(obj))
        return obj;
    node = js_mallocz(ctx, sizeof(*node));
    if (!node) {
        JS_FreeValue(ctx, obj);
        return JS_EXCEPTION;
    }
    node->doc = sweet_doc_clone(doc);
    node->id = id;
    JS_SetOpaque(obj, node);
    return obj;
}

static DocOpaque *doc_opaque(JSContext *ctx, JSValueConst this_val)
{
    return JS_GetOpaque2(ctx, this_val, doc_class_id);
}

static NodeOpaque *node_opaque(JSContext *ctx, JSValueConst this_val)
{
    return JS_GetOpaque2(ctx, this_val, node_class_id);
}

static WinOpaque *win_opaque(JSContext *ctx, JSValueConst this_val)
{
    return JS_GetOpaque2(ctx, this_val, win_class_id);
}

static void drop_listeners(JSRuntime *rt, uint64_t doc_id)
{
    Listener **link = &listeners;
    while (*link) {
        if ((*link)->doc_id == doc_id) {
            Listener *item = *link;
            *link = item->next;
            JS_FreeValueRT(rt, item->func);
            js_free_rt(rt, item);
        } else {
            link = &(*link)->next;
        }
    }
}

/* Listener functions close over the document, and the document finalizer is
   what releases those functions. That cycle is an external JS_DupValue, so
   QuickJS cannot collect it. Free every callback when the module dies. */
static void free_all_callbacks(JSRuntime *rt)
{
    Listener *items = listeners;
    CloseWait *pending = waits;
    listeners = NULL;
    waits = NULL;
    while (pending) {
        CloseWait *wait = pending;
        pending = wait->next;
        JS_FreeValueRT(rt, wait->resolve);
        js_free_rt(rt, wait);
    }
    while (items) {
        Listener *item = items;
        items = item->next;
        JS_FreeValueRT(rt, item->func);
        js_free_rt(rt, item);
    }
}

static void cleanup_finalizer(JSRuntime *rt, JSValue val)
{
    (void)val;
    free_all_callbacks(rt);
}

static void doc_finalizer(JSRuntime *rt, JSValue val)
{
    DocOpaque *doc = JS_GetOpaque(val, doc_class_id);
    if (!doc)
        return;
    drop_listeners(rt, sweet_doc_id(doc->doc));
    sweet_doc_free(doc->doc);
    js_free_rt(rt, doc);
}

static void node_finalizer(JSRuntime *rt, JSValue val)
{
    NodeOpaque *node = JS_GetOpaque(val, node_class_id);
    if (!node)
        return;
    sweet_doc_free(node->doc);
    js_free_rt(rt, node);
}

static void win_finalizer(JSRuntime *rt, JSValue val)
{
    WinOpaque *win = JS_GetOpaque(val, win_class_id);
    if (!win)
        return;
    sweet_window_close(win->token);
    sweet_doc_free(win->doc);
    js_free_rt(rt, win);
}

static JSValue js_doc_ctor(JSContext *ctx, JSValueConst new_target, int argc, JSValueConst *argv)
{
    char err[512];
    DocOpaque *box;
    JSValue proto, obj;
    SweetDoc *doc;

    (void)argc;
    (void)argv;
    err[0] = 0;
    doc = sweet_doc_new(err, sizeof(err));
    if (!doc)
        return throw_err(ctx, err);
    proto = JS_GetPropertyStr(ctx, new_target, "prototype");
    if (JS_IsException(proto)) {
        sweet_doc_free(doc);
        return JS_EXCEPTION;
    }
    obj = JS_NewObjectProtoClass(ctx, proto, doc_class_id);
    JS_FreeValue(ctx, proto);
    if (JS_IsException(obj)) {
        sweet_doc_free(doc);
        return JS_EXCEPTION;
    }
    box = js_mallocz(ctx, sizeof(*box));
    if (!box) {
        sweet_doc_free(doc);
        JS_FreeValue(ctx, obj);
        return JS_EXCEPTION;
    }
    box->doc = doc;
    JS_SetOpaque(obj, box);
    return obj;
}

static JSValue js_doc_body(JSContext *ctx, JSValueConst this_val)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    if (!doc)
        return JS_EXCEPTION;
    return new_node(ctx, doc->doc, sweet_body(doc->doc));
}

static JSValue js_doc_get_dark(JSContext *ctx, JSValueConst this_val)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    if (!doc)
        return JS_EXCEPTION;
    return JS_NewBool(ctx, sweet_dark(doc->doc));
}

static JSValue js_doc_set_dark(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    int dark;
    if (!doc)
        return JS_EXCEPTION;
    dark = JS_ToBool(ctx, val);
    if (dark < 0)
        return JS_EXCEPTION;
    sweet_set_dark(doc->doc, dark);
    return JS_UNDEFINED;
}

static JSValue js_doc_create(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv, int magic)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    const char *text = NULL;
    char err[512];
    uint32_t id = 0;
    int rc;
    if (!doc)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (read_cstr(ctx, argv[0], &text))
        return JS_EXCEPTION;
    err[0] = 0;
    if (magic == 0)
        rc = sweet_create_element(doc->doc, text, &id, err, sizeof(err));
    else if (magic == 1)
        rc = sweet_create_text(doc->doc, text, &id, err, sizeof(err));
    else
        rc = sweet_create_comment(doc->doc, text, &id, err, sizeof(err));
    JS_FreeCString(ctx, text);
    if (rc < 0)
        return throw_err(ctx, err);
    return new_node(ctx, doc->doc, id);
}

static JSValue bytes_to_u8(JSContext *ctx, uint8_t *ptr, size_t len)
{
    JSValue global, ctor, ab, arr;
    ab = JS_NewArrayBufferCopy(ctx, ptr, len);
    sweet_bytes_free(ptr, len);
    if (JS_IsException(ab))
        return ab;
    global = JS_GetGlobalObject(ctx);
    ctor = JS_GetPropertyStr(ctx, global, "Uint8Array");
    arr = JS_CallConstructor(ctx, ctor, 1, &ab);
    JS_FreeValue(ctx, ab);
    JS_FreeValue(ctx, ctor);
    JS_FreeValue(ctx, global);
    return arr;
}

static JSValue js_doc_render_png(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    int32_t width, height;
    uint8_t *bytes = NULL;
    size_t len = 0;
    char err[512];
    if (!doc)
        return JS_EXCEPTION;
    if (argc < 2)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (JS_ToInt32(ctx, &width, argv[0]) || JS_ToInt32(ctx, &height, argv[1]))
        return JS_EXCEPTION;
    if (width < 0 || height < 0)
        return JS_ThrowRangeError(ctx, "negative image size");
    err[0] = 0;
    if (sweet_render_png(doc->doc, (uint32_t)width, (uint32_t)height, &bytes, &len, err, sizeof(err)))
        return throw_err(ctx, err);
    return bytes_to_u8(ctx, bytes, len);
}

static JSValue js_doc_layout(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    DocOpaque *doc = doc_opaque(ctx, this_val);
    double width, height;
    SweetBox *boxes = NULL;
    size_t len = 0, i;
    char err[512];
    JSValue arr;
    if (!doc)
        return JS_EXCEPTION;
    if (argc < 2)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (JS_ToFloat64(ctx, &width, argv[0]) || JS_ToFloat64(ctx, &height, argv[1]))
        return JS_EXCEPTION;
    err[0] = 0;
    if (sweet_layout_boxes(doc->doc, width, height, &boxes, &len, err, sizeof(err)))
        return throw_err(ctx, err);
    arr = JS_NewArray(ctx);
    for (i = 0; i < len; i++) {
        JSValue obj = JS_NewObject(ctx);
        JS_SetPropertyStr(ctx, obj, "id", JS_NewUint32(ctx, boxes[i].id));
        if (boxes[i].parent < 0)
            JS_SetPropertyStr(ctx, obj, "parent", JS_NULL);
        else
            JS_SetPropertyStr(ctx, obj, "parent", JS_NewUint32(ctx, (uint32_t)boxes[i].parent));
        JS_SetPropertyStr(ctx, obj, "x", JS_NewFloat64(ctx, boxes[i].x));
        JS_SetPropertyStr(ctx, obj, "y", JS_NewFloat64(ctx, boxes[i].y));
        JS_SetPropertyStr(ctx, obj, "width", JS_NewFloat64(ctx, boxes[i].width));
        JS_SetPropertyStr(ctx, obj, "height", JS_NewFloat64(ctx, boxes[i].height));
        JS_SetPropertyUint32(ctx, arr, (uint32_t)i, obj);
    }
    sweet_box_free(boxes, len);
    return arr;
}

static JSValue node_get_string(JSContext *ctx, JSValueConst this_val,
                               char *(*read)(SweetDoc *, uint32_t, char *, size_t))
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    char err[512];
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    return take_cstr(ctx, read(node->doc, node->id, err, sizeof(err)), err);
}

static JSValue js_node_tag(JSContext *ctx, JSValueConst this_val)
{
    return node_get_string(ctx, this_val, sweet_tag_name);
}

static JSValue js_node_class(JSContext *ctx, JSValueConst this_val)
{
    return node_get_string(ctx, this_val, sweet_class_name);
}

static JSValue js_node_text(JSContext *ctx, JSValueConst this_val)
{
    return node_get_string(ctx, this_val, sweet_text_content);
}

static JSValue js_node_id_attr(JSContext *ctx, JSValueConst this_val)
{
    return node_get_string(ctx, this_val, sweet_id_attr);
}

static JSValue js_node_value(JSContext *ctx, JSValueConst this_val)
{
    return node_get_string(ctx, this_val, sweet_control_value);
}

static JSValue js_node_type(JSContext *ctx, JSValueConst this_val)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    char err[512];
    int kind;
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    kind = sweet_node_type(node->doc, node->id, err, sizeof(err));
    if (kind < 0)
        return throw_err(ctx, err);
    return JS_NewInt32(ctx, kind);
}

static JSValue js_node_node_id(JSContext *ctx, JSValueConst this_val)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    if (!node)
        return JS_EXCEPTION;
    return JS_NewUint32(ctx, node->id);
}

static JSValue node_set_string(JSContext *ctx, JSValueConst this_val, JSValueConst val,
                               int (*write)(SweetDoc *, uint32_t, const char *, char *, size_t))
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    const char *text = NULL;
    char err[512];
    int rc;
    if (!node)
        return JS_EXCEPTION;
    if (read_cstr(ctx, val, &text))
        return JS_EXCEPTION;
    err[0] = 0;
    rc = write(node->doc, node->id, text, err, sizeof(err));
    JS_FreeCString(ctx, text);
    if (rc < 0)
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_set_class(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    return node_set_string(ctx, this_val, val, sweet_set_class_name);
}

static JSValue js_node_set_text(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    return node_set_string(ctx, this_val, val, sweet_set_text_content);
}

static JSValue js_node_set_id(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    return node_set_string(ctx, this_val, val, sweet_set_id_attr);
}

static JSValue js_node_set_value(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    return node_set_string(ctx, this_val, val, sweet_set_control_value);
}

static JSValue js_node_disabled(JSContext *ctx, JSValueConst this_val)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    char err[512];
    int disabled;
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    disabled = sweet_disabled(node->doc, node->id, err, sizeof(err));
    if (disabled < 0)
        return throw_err(ctx, err);
    return JS_NewBool(ctx, disabled);
}

static JSValue js_node_set_disabled(JSContext *ctx, JSValueConst this_val, JSValueConst val)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    char err[512];
    int disabled;
    if (!node)
        return JS_EXCEPTION;
    disabled = JS_ToBool(ctx, val);
    if (disabled < 0)
        return JS_EXCEPTION;
    err[0] = 0;
    if (sweet_set_disabled(node->doc, node->id, disabled, err, sizeof(err)))
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_set_attribute(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    const char *name = NULL, *value = NULL;
    char err[512];
    int rc;
    if (!node)
        return JS_EXCEPTION;
    if (argc < 2)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (read_cstr(ctx, argv[0], &name))
        return JS_EXCEPTION;
    if (read_cstr(ctx, argv[1], &value)) {
        JS_FreeCString(ctx, name);
        return JS_EXCEPTION;
    }
    err[0] = 0;
    rc = sweet_set_attribute(node->doc, node->id, name, value, err, sizeof(err));
    JS_FreeCString(ctx, name);
    JS_FreeCString(ctx, value);
    if (rc < 0)
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_get_attribute(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    const char *name = NULL;
    char *value = NULL;
    char err[512];
    int rc;
    if (!node)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (read_cstr(ctx, argv[0], &name))
        return JS_EXCEPTION;
    err[0] = 0;
    rc = sweet_get_attribute(node->doc, node->id, name, &value, err, sizeof(err));
    JS_FreeCString(ctx, name);
    if (rc < 0)
        return throw_err(ctx, err);
    if (rc == 0)
        return JS_NULL;
    return take_cstr(ctx, value, err);
}

static JSValue js_node_remove_attribute(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    const char *name = NULL;
    char err[512];
    int rc;
    if (!node)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (read_cstr(ctx, argv[0], &name))
        return JS_EXCEPTION;
    err[0] = 0;
    rc = sweet_remove_attribute(node->doc, node->id, name, err, sizeof(err));
    JS_FreeCString(ctx, name);
    if (rc < 0)
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static int view_bytes(JSContext *ctx, JSValueConst value, const uint8_t **out, size_t *out_len, JSValue *held)
{
    size_t offset = 0, length = 0, bpe = 0, size = 0;
    uint8_t *buf;
    JSValue ab;
    *held = JS_UNINITIALIZED;
    ab = JS_GetTypedArrayBuffer(ctx, value, &offset, &length, &bpe);
    if (JS_IsException(ab)) {
        JSValue exc = JS_GetException(ctx);
        JS_FreeValue(ctx, exc);
        buf = JS_GetArrayBuffer(ctx, &size, value);
        if (!buf)
            return -1;
        *out = buf;
        *out_len = size;
        return 0;
    }
    buf = JS_GetArrayBuffer(ctx, &size, ab);
    if (!buf) {
        JS_FreeValue(ctx, ab);
        return -1;
    }
    if (offset > size || length > size - offset) {
        JS_FreeValue(ctx, ab);
        return -1;
    }
    *out = buf + offset;
    *out_len = length;
    *held = ab;
    return 0;
}

static JSValue js_node_set_image(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    const uint8_t *bytes = NULL;
    size_t len = 0;
    JSValue held = JS_UNINITIALIZED;
    uint32_t width = 0, height = 0;
    char err[512];
    JSValue obj;
    if (!node)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    if (view_bytes(ctx, argv[0], &bytes, &len, &held))
        return JS_ThrowTypeError(ctx, "image bytes must be a typed array");
    err[0] = 0;
    if (sweet_set_image(node->doc, node->id, bytes, len, &width, &height, err, sizeof(err))) {
        if (!JS_IsUninitialized(held))
            JS_FreeValue(ctx, held);
        return throw_err(ctx, err);
    }
    if (!JS_IsUninitialized(held))
        JS_FreeValue(ctx, held);
    obj = JS_NewObject(ctx);
    JS_SetPropertyStr(ctx, obj, "width", JS_NewUint32(ctx, width));
    JS_SetPropertyStr(ctx, obj, "height", JS_NewUint32(ctx, height));
    return obj;
}

static JSValue js_node_clear_image(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    char err[512];
    (void)argc;
    (void)argv;
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    if (sweet_clear_image(node->doc, node->id, err, sizeof(err)))
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_insert(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    NodeOpaque *child, *before = NULL;
    char err[512];
    int has_before = 0;
    if (!node)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    child = node_opaque(ctx, argv[0]);
    if (!child)
        return JS_EXCEPTION;
    if (!sweet_doc_same(node->doc, child->doc))
        return JS_ThrowTypeError(ctx, "node belongs to another document");
    if (argc >= 2 && !JS_IsUndefined(argv[1]) && !JS_IsNull(argv[1])) {
        before = node_opaque(ctx, argv[1]);
        if (!before)
            return JS_EXCEPTION;
        if (!sweet_doc_same(node->doc, before->doc))
            return JS_ThrowTypeError(ctx, "node belongs to another document");
        has_before = 1;
    }
    err[0] = 0;
    if (sweet_insert_before(node->doc, node->id, child->id, has_before, before ? before->id : 0, err, sizeof(err)))
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_remove(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    NodeOpaque *child;
    char err[512];
    if (!node)
        return JS_EXCEPTION;
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing argument");
    child = node_opaque(ctx, argv[0]);
    if (!child)
        return JS_EXCEPTION;
    if (!sweet_doc_same(node->doc, child->doc))
        return JS_ThrowTypeError(ctx, "node belongs to another document");
    err[0] = 0;
    if (sweet_remove_child(node->doc, node->id, child->id, err, sizeof(err)))
        return throw_err(ctx, err);
    return JS_UNDEFINED;
}

static JSValue js_node_children(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    uint32_t *ids = NULL;
    size_t len = 0, i;
    char err[512];
    JSValue arr;
    (void)argc;
    (void)argv;
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    if (sweet_children(node->doc, node->id, &ids, &len, err, sizeof(err)))
        return throw_err(ctx, err);
    arr = JS_NewArray(ctx);
    for (i = 0; i < len; i++)
        JS_SetPropertyUint32(ctx, arr, (uint32_t)i, new_node(ctx, node->doc, ids[i]));
    sweet_u32_free(ids, len);
    return arr;
}

static JSValue js_node_parent(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    uint32_t id = 0;
    char err[512];
    int rc;
    (void)argc;
    (void)argv;
    if (!node)
        return JS_EXCEPTION;
    err[0] = 0;
    rc = sweet_parent(node->doc, node->id, &id, err, sizeof(err));
    if (rc < 0)
        return throw_err(ctx, err);
    if (rc == 0)
        return JS_NULL;
    return new_node(ctx, node->doc, id);
}

static int known_event(const char *event, int window)
{
    if (!strcmp(event, "click") || !strcmp(event, "mousedown") ||
        !strcmp(event, "mouseup") || !strcmp(event, "mousemove"))
        return 1;
    if (window)
        return !strcmp(event, "close");
    return !strcmp(event, "input") || !strcmp(event, "change") || !strcmp(event, "keydown");
}

static JSValue add_listener(JSContext *ctx, SweetDoc *doc, int32_t node, JSValueConst event, JSValueConst func)
{
    const char *name = NULL;
    Listener *item;
    if (!JS_IsFunction(ctx, func))
        return JS_ThrowTypeError(ctx, "not a function");
    if (read_cstr(ctx, event, &name))
        return JS_EXCEPTION;
    if (!known_event(name, node < 0)) {
        JSValue err = JS_ThrowTypeError(ctx, "unknown event `%s`", name);
        JS_FreeCString(ctx, name);
        return err;
    }
    if (strlen(name) >= sizeof(item->event)) {
        JS_FreeCString(ctx, name);
        return JS_ThrowTypeError(ctx, "event name is too long");
    }
    item = js_mallocz(ctx, sizeof(*item));
    if (!item) {
        JS_FreeCString(ctx, name);
        return JS_EXCEPTION;
    }
    item->doc_id = sweet_doc_id(doc);
    item->node = node;
    memcpy(item->event, name, strlen(name) + 1);
    item->func = JS_DupValue(ctx, func);
    item->next = listeners;
    listeners = item;
    sweet_listen(doc, name);
    JS_FreeCString(ctx, name);
    return JS_UNDEFINED;
}

static JSValue js_node_on(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    NodeOpaque *node = node_opaque(ctx, this_val);
    if (!node)
        return JS_EXCEPTION;
    if (argc < 2)
        return JS_ThrowTypeError(ctx, "missing argument");
    return add_listener(ctx, node->doc, (int32_t)node->id, argv[0], argv[1]);
}

static const char *kind_name(uint32_t kind)
{
    switch (kind) {
    case SWEET_CLICK: return "click";
    case SWEET_MOUSEDOWN: return "mousedown";
    case SWEET_MOUSEUP: return "mouseup";
    case SWEET_MOUSEMOVE: return "mousemove";
    case SWEET_INPUT: return "input";
    case SWEET_KEYDOWN: return "keydown";
    case SWEET_CLOSE: return "close";
    default: return NULL;
    }
}

static void call_func(JSContext *ctx, JSValue func, JSValue arg)
{
    JSValue ret = JS_Call(ctx, func, JS_UNDEFINED, 1, &arg);
    if (JS_IsException(ret)) {
        JSValue exc = JS_GetException(ctx);
        const char *text = JS_ToCString(ctx, exc);
        fprintf(stderr, "sweet: %s\n", text ? text : "exception");
        if (text)
            JS_FreeCString(ctx, text);
        JS_FreeValue(ctx, exc);
    }
    JS_FreeValue(ctx, ret);
}

static void resolve_waits(JSContext *ctx, uint64_t token)
{
    CloseWait **link = &waits;
    while (*link) {
        if ((*link)->token == token) {
            CloseWait *wait = *link;
            JSValue undef = JS_UNDEFINED;
            *link = wait->next;
            call_func(ctx, wait->resolve, undef);
            JS_FreeValue(ctx, wait->resolve);
            js_free(ctx, wait);
        } else {
            link = &(*link)->next;
        }
    }
}

static void deliver(JSContext *ctx, const SweetEvent *ev)
{
    const char *name = kind_name(ev->kind);
    Listener *it;
    JSValue *funcs;
    JSValue event;
    int count = 0, n = 0, i;
    if (!name)
        return;
    for (it = listeners; it; it = it->next) {
        if (it->doc_id == ev->doc_id && it->node == ev->listen_on && !strcmp(it->event, name))
            count++;
    }
    funcs = js_malloc(ctx, sizeof(JSValue) * (count > 0 ? (size_t)count : 1));
    if (!funcs)
        return;
    for (it = listeners; it; it = it->next) {
        if (it->doc_id == ev->doc_id && it->node == ev->listen_on && !strcmp(it->event, name))
            funcs[n++] = JS_DupValue(ctx, it->func);
    }
    event = JS_NewObject(ctx);
    JS_SetPropertyStr(ctx, event, "type", JS_NewString(ctx, name));
    if (ev->target >= 0)
        JS_SetPropertyStr(ctx, event, "node_id", JS_NewUint32(ctx, (uint32_t)ev->target));
    else
        JS_SetPropertyStr(ctx, event, "node_id", JS_NULL);
    JS_SetPropertyStr(ctx, event, "x", JS_NewFloat64(ctx, ev->x));
    JS_SetPropertyStr(ctx, event, "y", JS_NewFloat64(ctx, ev->y));
    if (ev->has_key)
        JS_SetPropertyStr(ctx, event, "key", JS_NewString(ctx, ev->key));
    else
        JS_SetPropertyStr(ctx, event, "key", JS_NULL);
    for (i = 0; i < n; i++) {
        call_func(ctx, funcs[i], event);
        JS_FreeValue(ctx, funcs[i]);
    }
    JS_FreeValue(ctx, event);
    js_free(ctx, funcs);
    if (ev->kind == SWEET_CLOSE)
        resolve_waits(ctx, ev->token);
}

static JSValue js_win_ctor(JSContext *ctx, JSValueConst new_target, int argc, JSValueConst *argv)
{
    DocOpaque *doc;
    WinOpaque *box;
    JSValue proto, obj, title_v, width_v, height_v;
    const char *title_owned = NULL;
    const char *title = "sweet";
    double width = 960, height = 600;
    uint64_t token = 0;
    char err[512];
    if (argc < 1)
        return JS_ThrowTypeError(ctx, "missing document");
    doc = doc_opaque(ctx, argv[0]);
    if (!doc)
        return JS_EXCEPTION;
    if (argc >= 2 && JS_IsObject(argv[1])) {
        title_v = JS_GetPropertyStr(ctx, argv[1], "title");
        if (!JS_IsUndefined(title_v) && !JS_IsNull(title_v)) {
            title_owned = JS_ToCString(ctx, title_v);
            if (!title_owned) {
                JS_FreeValue(ctx, title_v);
                return JS_EXCEPTION;
            }
            title = title_owned;
        }
        JS_FreeValue(ctx, title_v);
        width_v = JS_GetPropertyStr(ctx, argv[1], "width");
        if (!JS_IsUndefined(width_v) && !JS_IsNull(width_v)) {
            if (JS_ToFloat64(ctx, &width, width_v)) {
                JS_FreeValue(ctx, width_v);
                if (title_owned)
                    JS_FreeCString(ctx, title_owned);
                return JS_EXCEPTION;
            }
        }
        JS_FreeValue(ctx, width_v);
        height_v = JS_GetPropertyStr(ctx, argv[1], "height");
        if (!JS_IsUndefined(height_v) && !JS_IsNull(height_v)) {
            if (JS_ToFloat64(ctx, &height, height_v)) {
                JS_FreeValue(ctx, height_v);
                if (title_owned)
                    JS_FreeCString(ctx, title_owned);
                return JS_EXCEPTION;
            }
        }
        JS_FreeValue(ctx, height_v);
    }
    err[0] = 0;
    if (sweet_window_open(doc->doc, title, width, height, &token, err, sizeof(err))) {
        if (title_owned)
            JS_FreeCString(ctx, title_owned);
        return throw_err(ctx, err);
    }
    if (title_owned)
        JS_FreeCString(ctx, title_owned);
    proto = JS_GetPropertyStr(ctx, new_target, "prototype");
    if (JS_IsException(proto)) {
        sweet_window_close(token);
        return JS_EXCEPTION;
    }
    obj = JS_NewObjectProtoClass(ctx, proto, win_class_id);
    JS_FreeValue(ctx, proto);
    if (JS_IsException(obj)) {
        sweet_window_close(token);
        return JS_EXCEPTION;
    }
    box = js_mallocz(ctx, sizeof(*box));
    if (!box) {
        sweet_window_close(token);
        JS_FreeValue(ctx, obj);
        return JS_EXCEPTION;
    }
    box->token = token;
    box->doc = sweet_doc_clone(doc->doc);
    JS_SetOpaque(obj, box);
    return obj;
}

static JSValue js_win_on(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    WinOpaque *win = win_opaque(ctx, this_val);
    if (!win)
        return JS_EXCEPTION;
    if (argc < 2)
        return JS_ThrowTypeError(ctx, "missing argument");
    return add_listener(ctx, win->doc, -1, argv[0], argv[1]);
}

static JSValue js_win_close(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    WinOpaque *win = win_opaque(ctx, this_val);
    (void)argc;
    (void)argv;
    if (!win)
        return JS_EXCEPTION;
    sweet_window_close(win->token);
    return JS_UNDEFINED;
}

static JSValue js_win_closed(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    WinOpaque *win = win_opaque(ctx, this_val);
    JSValue funcs[2], promise;
    CloseWait *wait;
    (void)argc;
    (void)argv;
    if (!win)
        return JS_EXCEPTION;
    promise = JS_NewPromiseCapability(ctx, funcs);
    if (JS_IsException(promise))
        return promise;
    wait = js_mallocz(ctx, sizeof(*wait));
    if (!wait) {
        JS_FreeValue(ctx, funcs[0]);
        JS_FreeValue(ctx, funcs[1]);
        JS_FreeValue(ctx, promise);
        return JS_EXCEPTION;
    }
    wait->token = win->token;
    wait->resolve = funcs[0];
    wait->next = waits;
    waits = wait;
    JS_FreeValue(ctx, funcs[1]);
    return promise;
}

static JSValue js_notify_fd(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    (void)argc;
    (void)argv;
    return JS_NewInt32(ctx, sweet_notify_fd());
}

static JSValue js_windows_alive(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    (void)this_val;
    (void)argc;
    (void)argv;
    return JS_NewInt32(ctx, sweet_windows_alive());
}

static JSValue js_drain(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv)
{
    SweetEvent ev;
    int closes = 0;
    (void)this_val;
    (void)argc;
    (void)argv;
    sweet_kick();
    while (sweet_poll(&ev)) {
        if (ev.kind == SWEET_CLOSE)
            closes++;
        deliver(ctx, &ev);
    }
    /* Callbacks close over the document, so their JS_DupValue keeps the
       document alive and QuickJS aborts on exit. Drop them once nothing
       is still on screen. Close handlers have already run. */
    if (closes && sweet_windows_alive() == 0)
        free_all_callbacks(JS_GetRuntime(ctx));
    return JS_NewInt32(ctx, closes);
}

static JSClassDef doc_class = { "Document", .finalizer = doc_finalizer };
static JSClassDef node_class = { "Node", .finalizer = node_finalizer };
static JSClassDef win_class = { "Window", .finalizer = win_finalizer };
static JSClassDef cleanup_class = { "SweetCleanup", .finalizer = cleanup_finalizer };

static const JSCFunctionListEntry doc_proto[] = {
    JS_CGETSET_DEF("body", js_doc_body, NULL),
    JS_CGETSET_DEF("dark", js_doc_get_dark, js_doc_set_dark),
    JS_CFUNC_MAGIC_DEF("createElement", 1, js_doc_create, 0),
    JS_CFUNC_MAGIC_DEF("createText", 1, js_doc_create, 1),
    JS_CFUNC_MAGIC_DEF("createComment", 1, js_doc_create, 2),
    JS_CFUNC_DEF("renderPng", 2, js_doc_render_png),
    JS_CFUNC_DEF("layoutBoxes", 2, js_doc_layout),
};

static const JSCFunctionListEntry node_proto[] = {
    JS_CGETSET_DEF("nodeId", js_node_node_id, NULL),
    JS_CGETSET_DEF("tagName", js_node_tag, NULL),
    JS_CGETSET_DEF("nodeType", js_node_type, NULL),
    JS_CGETSET_DEF("className", js_node_class, js_node_set_class),
    JS_CGETSET_DEF("textContent", js_node_text, js_node_set_text),
    JS_CGETSET_DEF("id", js_node_id_attr, js_node_set_id),
    JS_CGETSET_DEF("value", js_node_value, js_node_set_value),
    JS_CGETSET_DEF("disabled", js_node_disabled, js_node_set_disabled),
    JS_CFUNC_DEF("setAttribute", 2, js_node_set_attribute),
    JS_CFUNC_DEF("getAttribute", 1, js_node_get_attribute),
    JS_CFUNC_DEF("removeAttribute", 1, js_node_remove_attribute),
    JS_CFUNC_DEF("setImageEncoded", 1, js_node_set_image),
    JS_CFUNC_DEF("clearImage", 0, js_node_clear_image),
    JS_CFUNC_DEF("insertBefore", 2, js_node_insert),
    JS_CFUNC_DEF("removeChild", 1, js_node_remove),
    JS_CFUNC_DEF("children", 0, js_node_children),
    JS_CFUNC_DEF("parent", 0, js_node_parent),
    JS_CFUNC_DEF("on", 2, js_node_on),
};

static const JSCFunctionListEntry win_proto[] = {
    JS_CFUNC_DEF("on", 2, js_win_on),
    JS_CFUNC_DEF("close", 0, js_win_close),
    JS_CFUNC_DEF("closed", 0, js_win_closed),
};

static int module_init(JSContext *ctx, JSModuleDef *m)
{
    JSValue doc_proto_v, doc_ctor, node_proto_v, win_proto_v, win_ctor;

    JS_NewClassID(&doc_class_id);
    JS_NewClassID(&node_class_id);
    JS_NewClassID(&win_class_id);
    JS_NewClassID(&cleanup_class_id);
    JS_NewClass(JS_GetRuntime(ctx), doc_class_id, &doc_class);
    JS_NewClass(JS_GetRuntime(ctx), node_class_id, &node_class);
    JS_NewClass(JS_GetRuntime(ctx), win_class_id, &win_class);
    JS_NewClass(JS_GetRuntime(ctx), cleanup_class_id, &cleanup_class);

    doc_proto_v = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, doc_proto_v, doc_proto, countof(doc_proto));
    doc_ctor = JS_NewCFunction2(ctx, js_doc_ctor, "Document", 0, JS_CFUNC_constructor, 0);
    JS_SetConstructor(ctx, doc_ctor, doc_proto_v);
    JS_SetClassProto(ctx, doc_class_id, doc_proto_v);

    node_proto_v = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, node_proto_v, node_proto, countof(node_proto));
    JS_SetClassProto(ctx, node_class_id, node_proto_v);

    win_proto_v = JS_NewObject(ctx);
    JS_SetPropertyFunctionList(ctx, win_proto_v, win_proto, countof(win_proto));
    win_ctor = JS_NewCFunction2(ctx, js_win_ctor, "Window", 2, JS_CFUNC_constructor, 0);
    JS_SetConstructor(ctx, win_ctor, win_proto_v);
    JS_SetClassProto(ctx, win_class_id, win_proto_v);

    JS_SetModuleExport(ctx, m, "Document", doc_ctor);
    JS_SetModuleExport(ctx, m, "Window", win_ctor);
    JS_SetModuleExport(ctx, m, "notifyFd", JS_NewCFunction(ctx, js_notify_fd, "notifyFd", 0));
    JS_SetModuleExport(ctx, m, "windowsAlive", JS_NewCFunction(ctx, js_windows_alive, "windowsAlive", 0));
    JS_SetModuleExport(ctx, m, "drain", JS_NewCFunction(ctx, js_drain, "drain", 0));
    JS_SetModulePrivateValue(ctx, m, JS_NewObjectClass(ctx, cleanup_class_id));
    return 0;
}

JSModuleDef *sweet_js_init_module(JSContext *ctx, const char *module_name)
{
    JSModuleDef *m = JS_NewCModule(ctx, module_name, module_init);
    if (!m)
        return NULL;
    JS_AddModuleExport(ctx, m, "Document");
    JS_AddModuleExport(ctx, m, "Window");
    JS_AddModuleExport(ctx, m, "notifyFd");
    JS_AddModuleExport(ctx, m, "windowsAlive");
    JS_AddModuleExport(ctx, m, "drain");
    return m;
}
