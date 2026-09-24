<?php
// IMAGETYPE_* constants, image_type_to_mime_type(), image_type_to_extension().
foreach (['GIF', 'JPEG', 'PNG', 'SWF', 'PSD', 'BMP', 'TIFF_II', 'TIFF_MM', 'JPC', 'JP2', 'JPX', 'JB2', 'SWC',
    'IFF', 'WBMP', 'JPEG2000', 'XBM', 'ICO', 'WEBP', 'AVIF', 'HEIF', 'UNKNOWN', 'COUNT', 'SVG'] as $k) {
    echo "IMAGETYPE_$k = ", constant("IMAGETYPE_$k"), "\n";
}
for ($i = -1; $i <= 23; $i++) {
    echo $i, ': ', image_type_to_mime_type($i), ' ';
    var_dump(image_type_to_extension($i), image_type_to_extension($i, false));
}
var_dump(image_type_to_mime_type("3"), image_type_to_extension("2", 0));
try {
    image_type_to_mime_type("png");
} catch (\TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    image_type_to_extension([]);
} catch (\TypeError $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
