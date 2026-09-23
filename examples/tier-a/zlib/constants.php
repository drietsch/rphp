<?php
// ext/zlib's constants, ini entries and function list.
var_dump(extension_loaded('zlib'));
foreach (['FORCE_GZIP', 'FORCE_DEFLATE', 'ZLIB_ENCODING_RAW', 'ZLIB_ENCODING_GZIP', 'ZLIB_ENCODING_DEFLATE',
          'ZLIB_NO_FLUSH', 'ZLIB_PARTIAL_FLUSH', 'ZLIB_SYNC_FLUSH', 'ZLIB_FULL_FLUSH', 'ZLIB_BLOCK', 'ZLIB_FINISH',
          'ZLIB_FILTERED', 'ZLIB_HUFFMAN_ONLY', 'ZLIB_RLE', 'ZLIB_FIXED', 'ZLIB_DEFAULT_STRATEGY', 'ZLIB_VERSION',
          'ZLIB_VERNUM', 'ZLIB_OK', 'ZLIB_STREAM_END', 'ZLIB_NEED_DICT', 'ZLIB_ERRNO', 'ZLIB_STREAM_ERROR',
          'ZLIB_DATA_ERROR', 'ZLIB_MEM_ERROR', 'ZLIB_BUF_ERROR', 'ZLIB_VERSION_ERROR'] as $k) {
    echo $k, '=', var_export(constant($k), true), "\n";
}
foreach (['zlib.output_compression', 'zlib.output_compression_level', 'zlib.output_handler'] as $k) {
    var_dump(ini_get($k));
}
var_dump(zlib_get_coding_type());
foreach (['zlib_encode', 'zlib_decode', 'gzcompress', 'gzuncompress', 'gzdeflate', 'gzinflate', 'gzencode', 'gzdecode',
          'deflate_init', 'deflate_add', 'inflate_init', 'inflate_add', 'inflate_get_status', 'inflate_get_read_len',
          'gzopen', 'gzread', 'gzwrite', 'gzputs', 'gzgets', 'gzgetc', 'gzeof', 'gzclose', 'gzseek', 'gztell',
          'gzrewind', 'gzpassthru', 'gzfile', 'readgzfile', 'zlib_get_coding_type'] as $fn) {
    if (!function_exists($fn)) echo "missing $fn\n";
}
var_dump(class_exists('DeflateContext'), class_exists('InflateContext'));
