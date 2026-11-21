#include "game.h"

#include <stdlib.h>

typedef exports_parlando_game_mechanics_frame_t frame_t;
typedef exports_parlando_game_mechanics_role_t role_t;

/* Each component instance owns its pile and the seat entitled to move. */
static struct {
    uint8_t remaining;
    role_t turn;
} session;

/* Allocate a small result buffer which the generated ABI releases after lifting. */
static uint8_t *result_bytes(size_t length) {
    uint8_t *bytes = malloc(length);
    if (bytes == NULL) {
        __builtin_trap();
    }
    return bytes;
}

/* The compact view is [remaining, your_turn]. The pile fits in a positive fixint. */
static game_list_u8_t view_for(role_t seat) {
    uint8_t *bytes = result_bytes(3);
    bytes[0] = 0x92;
    bytes[1] = session.remaining;
    bytes[2] = session.remaining > 0 && seat == session.turn ? 0xc3 : 0xc2;
    return (game_list_u8_t){bytes, 3};
}

/* Emit both views and, on the final move, the shared [winner] record. */
static void write_frame(frame_t *frame) {
    *frame = (frame_t){
        .view_a = view_for(EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_A),
        .view_b = view_for(EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_B),
    };
    if (session.remaining == 0) {
        uint8_t *bytes = result_bytes(3);
        bytes[0] = 0x91;
        bytes[1] = 0xa1;
        bytes[2] = session.turn == EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_A ? 'A' : 'B';
        frame->completion.is_some = true;
        frame->completion.val = (game_list_u8_t){bytes, 3};
    }
}

/* Declare the public field names and their compact MessagePack representation. */
void exports_parlando_game_mechanics_describe(game_string_t *ret) {
#define VIEW_SCHEMA \
    "{\"type\":\"struct\",\"value\":[" \
    "{\"name\":\"remaining\",\"schema\":{\"type\":\"u64\"}}," \
    "{\"name\":\"your_turn\",\"schema\":{\"type\":\"bool\"}}]}"
    game_string_dup(ret,
        "{\"config\":{\"type\":\"struct\",\"value\":[]},"
        "\"action\":{\"type\":\"enum\",\"value\":{"
        "\"representation\":{\"type\":\"internal\",\"value\":{\"tag\":\"type\"}},"
        "\"variants\":["
        "{\"name\":\"take_one\",\"payload\":{\"type\":\"unit\"}},"
        "{\"name\":\"take_two\",\"payload\":{\"type\":\"unit\"}},"
        "{\"name\":\"take_three\",\"payload\":{\"type\":\"unit\"}}]}},"
        "\"view_a\":" VIEW_SCHEMA ",\"view_b\":" VIEW_SCHEMA ","
        "\"completion\":{\"type\":\"struct\",\"value\":["
        "{\"name\":\"winner\",\"schema\":{\"type\":\"string\"}}]},"
        "\"metadata\":{\"type\":\"null\"}}");
#undef VIEW_SCHEMA
}

/* Accept empty configuration and deterministically assign a pile of 12–15 to A. */
bool exports_parlando_game_mechanics_init(game_list_u8_t *config, uint64_t seed,
                                          frame_t *ret, game_string_t *err) {
    bool empty = config->len == 1 && config->ptr[0] == 0x90;
    game_list_u8_free(config);
    if (!empty) {
        game_string_dup(err, "invalid_config");
        return false;
    }
    session.remaining = 12 + seed % 4;
    session.turn = EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_A;
    write_frame(ret);
    return true;
}

/* This game emits no checkpoints. Parlando recovers by replaying accepted moves. */
bool exports_parlando_game_mechanics_restore(game_list_u8_t *checkpoint,
                                             game_string_t *err) {
    game_list_u8_free(checkpoint);
    game_string_dup(err, "unsupported_checkpoint");
    return false;
}

/* Validate the actor and compact action before updating the pile and turn. */
bool exports_parlando_game_mechanics_step(role_t actor, game_list_u8_t *action,
                                          frame_t *ret, game_string_t *err) {
    // The host emits these three unit enum tags as one-byte positive fixints.
    uint8_t tag = action->len == 1 ? action->ptr[0] : UINT8_MAX;
    game_list_u8_free(action);
    const char *rejection = NULL;
    if (tag > 2) {
        rejection = "invalid_action";
    } else if (session.remaining == 0) {
        rejection = "game_over";
    } else if (actor != session.turn) {
        rejection = "wrong_turn";
    } else if (tag + 1 > session.remaining) {
        rejection = "too_many_tokens";
    }
    if (rejection != NULL) {
        game_string_dup(err, rejection);
        return false;
    }

    session.remaining -= tag + 1;
    if (session.remaining > 0) {
        session.turn = actor == EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_A
            ? EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_B
            : EXPORTS_PARLANDO_GAME_MECHANICS_ROLE_A;
    }
    write_frame(ret);
    return true;
}
