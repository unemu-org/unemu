// SPDX-License-Identifier: AGPL-3.0-or-later
/* Native vectors from Nuked-OPL3 1.8, commit 765ec962.
 * Link this driver to the unmodified upstream opl3.c; no resampler or buffer.
 * Input: frame count, then sample-index / hex-register / hex-value triples.
 * Output: signed 16-bit little-endian A/B/C/D frames, independent of host endian.
 */
#include <stdio.h>
#include <stdint.h>
#include "opl3.h"
int main(void) {
    opl3_chip chip;
    unsigned frames, at, reg, value;
    if (scanf("%u", &frames) != 1) return 1;
    int pending = scanf("%u %x %x", &at, &reg, &value) == 3;
    OPL3_Reset(&chip, 49716);
    for (unsigned n = 0; n < frames; ++n) {
        while (pending && at == n) {
            OPL3_WriteReg(&chip, reg, value);
            pending = scanf("%u %x %x", &at, &reg, &value) == 3;
        }
        int16_t pcm[4];
        OPL3_Generate4Ch(&chip, pcm);
        for (unsigned c = 0; c < 4; ++c) {
            uint16_t v = (uint16_t)pcm[c];
            putchar(v & 255);
            putchar(v >> 8);
        }
    }
    return pending || ferror(stdout);
}
