<?php
// Tier-A differential: the deterministic part of info.c / basic_functions.c
// — extension queries, version strings, gc_* state, ini_get_all for one
// extension, include_path, env round trips, connection status, and the
// shapes (not the values) of the platform / clock functions.

var_dump(phpversion(), phpversion("standard"), phpversion("json"), phpversion("pcre"), phpversion("nope"));
var_dump(zend_version());
var_dump(extension_loaded("standard"), extension_loaded("STANDARD"), extension_loaded("nope"), extension_loaded("Core"), extension_loaded("json"), extension_loaded("random"));
var_dump(get_extension_funcs("nope"));
$ctype = get_extension_funcs("ctype");
sort($ctype);
print_r($ctype);
var_dump(in_array("strlen", get_extension_funcs("Core")), in_array("strrev", get_extension_funcs("standard")), in_array("zend_version", get_extension_funcs("Core")), in_array("mt_rand", get_extension_funcs("random")), in_array("shuffle", get_extension_funcs("standard")));
var_dump(in_array("standard", get_loaded_extensions()), in_array("Core", get_loaded_extensions()));

var_dump(gc_enabled(), gc_collect_cycles(), gc_mem_caches() >= 0);
gc_disable();
var_dump(gc_enabled());
gc_enable();
var_dump(gc_enabled());
$status = gc_status();
var_dump($status["running"], $status["runs"], $status["collected"], $status["threshold"], count($status));

var_dump(php_ini_loaded_file(), php_ini_scanned_files(), get_cfg_var("nope"));
print_r(ini_get_all("pcre"));
print_r(ini_get_all("pcre", false));
var_dump(ini_get_all("nope"));
var_dump(count(ini_get_all()) > 50);
$all = ini_get_all(null, true);
print_r($all["precision"]);
var_dump(ini_get_all("standard", false)["assert.active"]);

$old = set_include_path("/x:/y");
var_dump(get_include_path(), ini_get("include_path"));
var_dump(set_include_path($old) === "/x:/y");

var_dump(putenv("RPHP_TIER_A=bar"), getenv("RPHP_TIER_A"), putenv("RPHP_TIER_A"), getenv("RPHP_TIER_A"));
var_dump(putenv("RPHP_EMPTY="), getenv("RPHP_EMPTY"), getenv("HOME") !== false, is_array(getenv()), getenv("NOPE_NOT_SET"));
var_dump(putenv("A_B=1"), putenv("A_B=2"), getenv("A_B"));

var_dump(connection_status(), connection_aborted(), ignore_user_abort(), ignore_user_abort(true), ignore_user_abort(), ignore_user_abort(false), ignore_user_abort());

// Platform / clock values: only their shape is compared.
var_dump(php_uname("s") === constant('PHP_OS'), strlen(php_uname("n")) > 0, strlen(php_uname("r")) > 0, strlen(php_uname("m")) > 0, strlen(php_uname()) > 10);
var_dump(getmypid() > 0, getmyuid() >= 0, getmygid() >= 0, getmyinode() > 0, getlastmod() > 0, strlen(get_current_user()) > 0);
var_dump(memory_get_usage() > 0, memory_get_peak_usage(true) > 0, memory_get_usage(true) >= memory_get_usage());
var_dump(strlen(sys_get_temp_dir()) > 0, substr(sys_get_temp_dir(), -1) !== "/");
var_dump(strlen(uniqid()), strlen(uniqid("p_")), strlen(uniqid("", true)), uniqid() !== uniqid());
$mt = microtime();
var_dump(strlen($mt) > 12, microtime(true) > 1000000000, time() > 1000000000, gettimeofday(true) > 1000000000);
$tod = gettimeofday();
var_dump(count($tod), $tod["sec"] > 1000000000, $tod["minuteswest"], $tod["dsttime"]);
$hr = hrtime();
var_dump(count($hr), hrtime(true) > 0);
var_dump(usleep(1), sleep(0), time_nanosleep(0, 1));
var_dump(putenv("=x"));
