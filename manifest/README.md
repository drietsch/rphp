# PHP oracle manifest

Machine-readable dump of the installed stock PHP (functions with full arginfo,
classes/methods, constants by category, ini directives, extensions), produced
by

```
php -n tools/manifest/dump.php manifest/php-8.5.0
```

and consumed by `cargo xtask gen` (per-extension `arginfo.rs`/`consts.rs`/
`ini.rs`/`classes.rs` tables of `rphp_ext_api` statics) and `cargo xtask
coverage`. One directory per PHP version (`php-8.5.0/`), each with its own
README stating the exact build string. Always run the dump with `-n` so no
php.ini leaks in: the ini file then holds compiled-in defaults.
