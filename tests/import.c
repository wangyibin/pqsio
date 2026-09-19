#include "pqsio.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int32_t result(const uint8_t *data, size_t size, void *user) {
    char *json = malloc(size + 1);
    if (!json) return -1;
    memcpy(json, data, size);
    json[size] = '\0';
    const char *expected = user;
    int seen = strstr(json, expected) != NULL;
    free(json);
    return seen ? 0 : -1;
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    char expected[] = "\"q0_records\":1";
    int32_t status = pqsio_import_json(argv[1], argv[2], "paf2pairs",
        2, 1, 0, 2, SIZE_MAX, 1, NULL, 0, NULL, 0, result, expected);
    if (status != 0) {
        fprintf(stderr, "%s\n", pqsio_last_error());
        return 1;
    }
    /* A second import must preserve the published dataset. */
    status = pqsio_import_json(argv[1], argv[2], "paf2pairs",
        2, 1, 0, 2, SIZE_MAX, 1, NULL, 0, NULL, 0, result, expected);
    if (status != -2) return 1;
    size_t capacity = strlen(argv[2]) + sizeof(".cool");
    char *cool = malloc(capacity);
    if (!cool) return 1;
    snprintf(cool, capacity, "%s.cool", argv[2]);
    char cool_expected[] = "\"sum\":1";
    status = pqsio_pairs2cool_json(argv[2], cool, 10, 2, 1, 0, 1,
        NULL, NULL, result, cool_expected);
    if (status != 0) {
        fprintf(stderr, "%s\n", pqsio_last_error());
        free(cool);
        return 1;
    }
    status = pqsio_pairs2cool_json(argv[2], cool, 10, 2, 1, 0, 1,
        NULL, NULL, result, cool_expected);
    free(cool);
    return status == -2 ? 0 : 1;
}
