#include <node_api.h>

/* A compilable fixture, not an MCP server or a production addon. */
static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  double left, right;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_double(env, argv[0], &left) != napi_ok ||
      napi_get_value_double(env, argv[1], &right) != napi_ok ||
      napi_create_double(env, left + right, &result) != napi_ok) {
    napi_throw_type_error(env, NULL, "Expected two numbers");
    return NULL;
  }
  return result;
}

static napi_value initialize(napi_env env, napi_value exports) {
  napi_value function;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "add", function) != napi_ok) {
    napi_throw_error(env, NULL, "Cannot initialize addon");
    return NULL;
  }
  return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, initialize)
