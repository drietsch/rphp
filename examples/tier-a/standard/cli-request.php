<?php
// Functions that have nothing to work on in a CLI run.
function t(callable $f) {
    try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
t(fn() => is_int(realpath_cache_size()));
t(fn() => is_array(realpath_cache_get()));
t(fn() => get_browser());
t(fn() => get_browser('Mozilla/5.0', true));
t(fn() => get_browser(null, false));
t(fn() => output_add_rewrite_var('a', 'b'));
t(fn() => output_add_rewrite_var('c', 'd'));
t(fn() => output_reset_rewrite_vars());
t(fn() => output_reset_rewrite_vars());
t(fn() => request_parse_body());
t(fn() => request_parse_body([]));
t(fn() => request_parse_body(null));
t(fn() => request_parse_body(['max_input_vars' => 3, 'post_max_size' => '1M']));
t(fn() => request_parse_body(['bogus' => 3]));
t(fn() => request_parse_body(['post_max_size' => '1x']));
t(fn() => request_parse_body('x'));
