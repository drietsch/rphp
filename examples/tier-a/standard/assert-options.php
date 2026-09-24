<?php
function t(callable $f) {
    try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
var_dump(ASSERT_ACTIVE, ASSERT_CALLBACK, ASSERT_BAIL, ASSERT_WARNING, ASSERT_EXCEPTION);
t(fn() => assert_options(ASSERT_ACTIVE));
t(fn() => assert_options(ASSERT_ACTIVE, 0));
t(fn() => assert_options(ASSERT_ACTIVE));
t(fn() => ini_get('assert.active'));
t(fn() => assert_options(ASSERT_ACTIVE, "on"));
t(fn() => assert_options(ASSERT_ACTIVE));
t(fn() => assert_options(ASSERT_BAIL));
t(fn() => assert_options(ASSERT_BAIL, true));
t(fn() => assert_options(ASSERT_BAIL));
t(fn() => assert_options(ASSERT_BAIL, 0));
t(fn() => assert_options(ASSERT_WARNING));
t(fn() => assert_options(ASSERT_WARNING, "0"));
t(fn() => assert_options(ASSERT_WARNING));
t(fn() => assert_options(ASSERT_EXCEPTION));
t(fn() => assert_options(ASSERT_EXCEPTION, 1));
t(fn() => assert_options(ASSERT_CALLBACK));
t(fn() => assert_options(ASSERT_CALLBACK, 'strlen'));
t(fn() => assert_options(ASSERT_CALLBACK));
t(fn() => assert_options(ASSERT_CALLBACK, null));
t(fn() => assert_options(ASSERT_CALLBACK));
t(fn() => assert_options(99));
t(fn() => assert_options(0));
t(fn() => assert_options(ASSERT_ACTIVE, []));
