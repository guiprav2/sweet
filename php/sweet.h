#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
typedef struct Document Document;
Document *sweet_document_new(char **error);
void sweet_document_free(Document *document);
uint32_t sweet_body(const Document *document);
uint32_t sweet_create_element(const Document *document, const char *tag, char **error);
uint32_t sweet_create_text(const Document *document, const char *text, char **error);
bool sweet_append_child(const Document *document, uint32_t parent, uint32_t child, char **error);
bool sweet_set_class(const Document *document, uint32_t id, const char *value, char **error);
bool sweet_set_text(const Document *document, uint32_t id, const char *value, char **error);
bool sweet_set_attribute(const Document *document, uint32_t id, const char *name, const char *value, char **error);
char *sweet_text_content(const Document *document, uint32_t id, char **error);
uint8_t *sweet_render_png(const Document *document, uint32_t width, uint32_t height, size_t *length, char **error);
void sweet_string_free(char *value);
void sweet_bytes_free(uint8_t *value, size_t length);
