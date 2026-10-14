#include <dlfcn.h>
#include <node.h>

extern "C" int sweet_node_start(const char *script_path) {
  char *argv[] = {const_cast<char *>("node"), const_cast<char *>(script_path)};
  return node::Start(2, argv);
}

extern "C" const char *sweet_node_library_path() {
  static const char *path = nullptr;
  if (path != nullptr) return path;

  Dl_info info{};
  if (dladdr(reinterpret_cast<void *>(&sweet_node_library_path), &info) == 0) {
    return nullptr;
  }
  path = info.dli_fname;
  return path;
}
