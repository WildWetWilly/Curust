/* Reads PNG files the way CuraEngine's MeshUtils::loadTextureFromPngData does (libpng simplified
 * API in the file's own format, then png_read_info + png_get_text) and prints the result, for
 * comparison with crates/engine/examples/png_simplified_driver.rs.
 * build: cc -O2 png_simplified_driver.c -lpng -o png_simplified_driver */
#include <png.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct ctx { const unsigned char* data; size_t size, offset; };

static void read_fn(png_structp p, png_bytep out, png_size_t n)
{
    struct ctx* c = (struct ctx*)png_get_io_ptr(p);
    if (c->offset + n > c->size) png_error(p, "Read beyond end of buffer");
    memcpy(out, c->data + c->offset, n);
    c->offset += n;
}

static unsigned long long fnv(const unsigned char* d, size_t n)
{
    unsigned long long h = 1469598103934665603ULL;
    for (size_t i = 0; i < n; i++) { h ^= d[i]; h *= 1099511628211ULL; }
    return h;
}

static void one(const char* path)
{
    FILE* f = fopen(path, "rb");
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    rewind(f);
    unsigned char* data = malloc(size);
    fread(data, 1, size, f);
    fclose(f);

    png_image img;
    memset(&img, 0, sizeof img);
    img.version = PNG_IMAGE_VERSION;
    if (!png_image_begin_read_from_memory(&img, data, size)) { printf("%s FAIL begin\n", path); free(data); return; }
    size_t bufsize = PNG_IMAGE_SIZE(img);
    unsigned char* buf = malloc(bufsize ? bufsize : 1);
    if (!png_image_finish_read(&img, NULL, buf, 0, NULL) || bufsize == 0) { printf("%s FAIL finish\n", path); free(buf); free(data); return; }

    png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, NULL, NULL, NULL);
    png_infop info = png_create_info_struct(png);
    if (setjmp(png_jmpbuf(png))) { printf("%s FAIL text\n", path); return; }
    struct ctx c = { data, (size_t)size, 0 };
    png_set_read_fn(png, &c, read_fn);
    png_read_info(png, info);
    png_textp text;
    int num_text;
    int r = png_get_text(png, info, &text, &num_text);
    printf("%s OK %u %u %u %016llx texts=%d", path, img.width, img.height,
           (unsigned)(PNG_IMAGE_SAMPLE_COMPONENT_SIZE(img.format) * PNG_IMAGE_SAMPLE_CHANNELS(img.format)), fnv(buf, bufsize), r > 0 ? num_text : 0);
    for (int i = 0; r > 0 && i < num_text; i++)
        printf(" [%s:%016llx:%zu]", text[i].key, fnv((unsigned char*)text[i].text, text[i].text_length), text[i].text_length);
    printf("\n");
    png_destroy_read_struct(&png, &info, NULL);
    free(buf);
    free(data);
}

int main(int argc, char** argv)
{
    for (int i = 1; i < argc; i++) one(argv[i]);
    return 0;
}
