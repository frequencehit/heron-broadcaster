// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Generated from openapi.json by `npm run generate-api`. Do not edit.

export interface paths {
    "/api/v1/artist-names/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /**
         * Moves a name to another artist (or to a new one): the tracks tagged
         *     with it follow. Operators and admins.
         */
        put: operations["move_name"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artist-suggestions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Links waiting for review. */
        get: operations["list_suggestions"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artist-suggestions/musicbrainz": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Asks heron-librarian to look up the MusicBrainz relationships ("member
         *     of", "is person", …) between the artists that have a MusicBrainz ID;
         *     they become suggestions. Operators and admins.
         */
        post: operations["look_up_relations"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artist-suggestions/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Accepts or dismisses a suggestion. Operators and admins. */
        post: operations["review_suggestion"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artists": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Artists, by name. */
        get: operations["list_artists"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artists/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_artist"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /** Renames an artist, or sets its MusicBrainz ID. Operators and admins. */
        patch: operations["patch_artist"];
        trace?: never;
    };
    "/api/v1/artists/{id}/merge": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Merges another artist into this one: its names, tracks, similar
         *     artists and restrictions. Operators and admins.
         */
        post: operations["merge_artist"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/artists/{id}/similar/{other}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /**
         * Marks two artists similar: separation treats them as the same artist.
         *     Operators and admins.
         */
        put: operations["add_similar"];
        post?: never;
        /** Operators and admins. */
        delete: operations["remove_similar"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/login": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Signs in: answers with the user and sets the `heron_session` cookie. */
        post: operations["login"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/logout": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Signs out: ends the session and clears the cookie. */
        post: operations["logout"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/me": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The signed-in user. */
        get: operations["me"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/password": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Changes your password. Your other sessions are signed out. */
        put: operations["change_password"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/auth/preferences": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Saves your preferences: the web UI's theme, language and width. */
        put: operations["set_preferences"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/categories": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_categories"];
        put?: never;
        /** Operators and admins. */
        post: operations["create_category"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/categories/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_category"];
        /**
         * Renames, recolors a category, changes its type, its separation or
         *     whether the FX player offers it. Operators and admins.
         */
        put: operations["update_category"];
        post?: never;
        /**
         * Deletes a category and its restrictions; its tracks stay in the
         *     library. Refused while a clock uses it. Operators and admins.
         */
        delete: operations["delete_category"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/category-types": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Every category type, in the order they were created. */
        get: operations["list_types"];
        put?: never;
        /** Operators and admins. */
        post: operations["create_type"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/category-types/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Renames a type, or changes its rules. Operators and admins. */
        put: operations["update_type"];
        post?: never;
        /** Deletes a type no category uses. Operators and admins. */
        delete: operations["delete_type"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/clocks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_clocks"];
        put?: never;
        /** Operators and admins. */
        post: operations["create_clock"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/clocks/preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * A clock as it will sound, from its slots (saved or not): expected
         *     starts and lengths, and the categories running short.
         */
        post: operations["preview_clock"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/clocks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_clock"];
        /** Renames a clock and replaces its slots. Operators and admins. */
        put: operations["update_clock"];
        post?: never;
        /**
         * Deletes a clock; the hours it was scheduled at rotate the whole library
         *     until another clock is scheduled. Operators and admins.
         */
        delete: operations["delete_clock"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/encoder-profiles": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_profiles"];
        put?: never;
        /** Adds an encoder profile. Admins only. */
        post: operations["create_profile"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/encoder-profiles/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_profile"];
        /**
         * Replaces an encoder profile. Outputs using it restart with the new
         *     encoding. Admins only.
         */
        put: operations["update_profile"];
        post?: never;
        /** Deletes an encoder profile no output uses. Admins only. */
        delete: operations["delete_profile"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/engine": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["engine_status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/history": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Plays, most recent first. */
        get: operations["list_history"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/labels": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Every label, with how many tracks have it. */
        get: operations["list_labels"];
        put?: never;
        /** Operators and admins. */
        post: operations["create_label"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/labels/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Renames or recolors a label. Operators and admins. */
        put: operations["update_label"];
        post?: never;
        /** Deletes a label: its tracks lose it. Operators and admins. */
        delete: operations["delete_label"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/labels/{id}/merge": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Merges a label into another: its tracks get the other label, and it is
         *     deleted. Operators and admins.
         */
        post: operations["merge_label"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/batches": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The most recent batches of file operations, newest first. */
        get: operations["list_batches"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/batches/{id}/undo": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Reverts a batch: files go back to their previous paths and get their
         *     previous tags back. Nothing is overwritten: a file whose previous path
         *     is taken stays where it is (see the task's report). Admins.
         */
        post: operations["undo_batch"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/import/apply": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Applies an import: sets each track's values, puts tracks in categories,
         *     labels them, disables them, as one transaction that can be undone.
         *     Admins only.
         */
        post: operations["apply"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/import/preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Matches an export's rows to library tracks and says what an import would
         *     change. Writes nothing. Admins only.
         */
        post: operations["preview"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/imports": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The last imports, newest first. Admins only. */
        get: operations["list_imports"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/imports/{id}/undo": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Undoes an import: every track gets back what it had before it, as one
         *     transaction. Categories and labels it created stay. Admins only.
         */
        post: operations["undo"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/musicbrainz-details": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Fetches the programming details of identified tracks from MusicBrainz
         *     (album, record label, ISRC, first release year, genres), one release a
         *     second. From then on, new tracks are looked up in the background.
         *     Operators and admins.
         */
        post: operations["fetch_details"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/organize": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Asks for a dry run of organizing the library with the layout in the
         *     library settings: the task's `plan` lists every move, nothing changes.
         *     Admins.
         */
        post: operations["plan_organization"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/organize/{plan_task_id}/apply": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Carries out the moves of a finished dry run, as one batch that can be
         *     undone. Tracks moved since the dry run are skipped. Admins.
         */
        post: operations["apply_organization"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/review": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Tracks whose identification was not confident enough to apply, with the
         *     releases they may be on.
         */
        get: operations["list_review"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/status": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Import and analysis progress. No sign-in needed. */
        get: operations["library_status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/tasks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The most recent library tasks, newest first. */
        get: operations["list_tasks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/tasks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** A library task, with its result once done. */
        get: operations["get_task"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/trash": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Tracks in the trash, most recently deleted first. */
        get: operations["list_trash"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/trash/empty": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Purges the whole trash now: the tracks and their files are removed for
         *     good; their plays keep their artist and title. Admins.
         */
        post: operations["empty_trash"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/library/trash/{batch_id}/restore": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Restores the tracks of a deletion (undoes its batch): their files go
         *     back where they were, with their analysis, categories and history. A
         *     path taken since is reported, and that track stays in the trash.
         *     Operators and admins.
         */
        post: operations["restore_batch"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/days": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Totals per hour, day, week or month: distinct listeners, peak, average,
         *     listening hours, sessions. Today's change until midnight.
         */
        get: operations["days"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/hours-of-day": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Each hour of the day (station time), averaged over the days of the
         *     period.
         */
        get: operations["hours_of_day"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/now": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Each output's listeners at the last minute, and the station's. */
        get: operations["now"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/players": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Sessions and listening hours by player (user agent), most listened
         *     first.
         */
        get: operations["players"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/samples": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Listeners minute by minute. Minutes no server could be read are
         *     missing.
         */
        get: operations["samples"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/listeners/tracks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Tracks of the period, those listeners left the most first (by
         *     departure rate, or by the change of listeners without sessions).
         */
        get: operations["tracks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/live": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Opens the live WebSocket. Messages are [`LiveMessage`]s. */
        get: operations["live"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/outputs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Outputs with their live state. */
        get: operations["list_outputs"];
        put?: never;
        /**
         * Adds an output; the engine starts streaming to it if it is enabled.
         *     Admins only.
         */
        post: operations["create_output"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/outputs/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_output"];
        /**
         * Replaces an output. Credentials left out of `config` are kept, and so
         *     are the statistics settings when `statistics` is left out. Admins only.
         */
        put: operations["update_output"];
        post?: never;
        /** Deletes an output; the engine stops streaming to it. Admins only. */
        delete: operations["delete_output"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Decks, queue and outputs, read from the engine now. */
        get: operations["get_player"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player/fx": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Plays a library track on the FX player, over the music. Replaces the
         *     FX playing, if any.
         */
        post: operations["play_fx"];
        /** Fades out the FX player. */
        delete: operations["stop_fx"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player/levels": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Peak and RMS levels (dBFS) of the master bus and each deck. */
        get: operations["get_levels"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player/pause": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Fades out and holds the decks; the stream goes on with silence. */
        post: operations["pause"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/player/skip": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Fades out the track on air and starts the next one. */
        post: operations["skip"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/playout/transition": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Plans the transition between two tracks the way the engine would, for
         *     previews. Nothing is played.
         */
        post: operations["preview_transition"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/public/history": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * What was played, most recent first, the FX player and tracks that send no
         *     metadata (jingles) left out. No sign-in needed; any website may read it.
         */
        get: operations["public_history"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/public/now-playing": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The track on air; while a track that sends no metadata (a jingle) is on
         *     air, the last one that does. No sign-in needed; any website may read it.
         */
        get: operations["now_playing"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/queue": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The tracks waiting to play, in order: the cued one first. */
        get: operations["get_queue"];
        put?: never;
        /**
         * Queues library tracks. Missing files and tracks with nothing audible are
         *     refused.
         */
        post: operations["enqueue"];
        /**
         * Empties the queue, cued track included. The track on air plays on (and
         *     autofill, when on, queues new tracks).
         */
        delete: operations["clear_queue"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/queue/{entry_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Takes an entry out of the queue. The track on air cannot be removed: skip it. */
        delete: operations["remove_entry"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/queue/{entry_id}/position": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Moves a waiting entry. */
        put: operations["move_entry"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/restrictions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_restrictions"];
        put?: never;
        /** Operators and admins. */
        post: operations["create_restriction"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/restrictions/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Operators and admins. */
        put: operations["update_restriction"];
        post?: never;
        /** Operators and admins. */
        delete: operations["delete_restriction"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/rotation/picks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The selection log, most recent first. */
        get: operations["list_picks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/rotation/simulate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Generates programming offline, without touching the queue, and checks
         *     it: separation as relaxed at each pick's step, restrictions always.
         *     Operators and admins.
         */
        post: operations["simulate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/schedule": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Which clock runs when; hours left out rotate the whole library. */
        get: operations["get_schedule"];
        /** Replaces the whole schedule. Operators and admins. */
        put: operations["put_schedule"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/analysis": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Gap Killer and crossfade point detection, used at import. */
        get: operations["get_analysis"];
        /**
         * Replaces the analysis settings. Every track is analyzed again with the
         *     new settings, in the background. Admins only.
         */
        put: operations["put_analysis"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/library": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Identification and folder layout, used by `heron-librarian`. */
        get: operations["get_library"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /** Changes some library settings; the others are kept. Admins only. */
        patch: operations["patch_library"];
        trace?: never;
    };
    "/api/v1/settings/playout": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Crossfades and loudness normalization. */
        get: operations["get_playout"];
        /**
         * Replaces the playout settings (missing fields take the defaults) and
         *     pushes them to the engine. Admins only.
         */
        put: operations["put_playout"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/programming": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** What counts as a play, eras, and the mood and sound code scales. */
        get: operations["get_programming"];
        /**
         * Replaces the programming settings (missing fields take the defaults).
         *     Tracks keep their mood and sound code when a scale changes. Operators
         *     and admins.
         */
        put: operations["put_programming"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/queue": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_queue_settings"];
        /** Replaces the queue manager settings. Operators and admins. */
        put: operations["put_queue_settings"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/rotation": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Separation rules, the fallback category and weights. */
        get: operations["get_rotation_settings"];
        /**
         * Replaces the rotation settings (missing fields take the defaults).
         *     Operators and admins.
         */
        put: operations["put_rotation_settings"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/station": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_station"];
        /** Replaces the station settings. Admins only. */
        put: operations["put_station"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/settings/statistics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Listener statistics: the minimum session, how long sessions are kept. */
        get: operations["get_statistics"];
        /**
         * Replaces the listener statistics settings (missing fields take the
         *     defaults). A new minimum session recomputes the totals the sessions
         *     kept still cover. Admins only.
         */
        put: operations["put_statistics"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The buffer zone: groups with files to import or discard, with their
         *     identification, flags and planned paths.
         */
        get: operations["get_buffer"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/discard": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Removes files from the buffer zone (uploads in progress too). They never
         *     reached the library. Files being imported are left alone. Operators and
         *     admins.
         */
        post: operations["discard_files"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/files/preparation": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Prepares files of the buffer zone for when they are imported: the
         *     categories, labels, mood and sound code their tracks get. Files being
         *     imported, or no longer in the buffer, are refused. Operators and admins.
         */
        post: operations["prepare_files"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/files/tags": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Edits the tags files of the buffer zone are imported with (title,
         *     artist, album, album artist, year, track, disc), on top of those they
         *     get from their identification, a release picked by hand or their own
         *     tags. heron-librarian works their path out again. Files being imported,
         *     or no longer in the buffer, are refused. Operators and admins.
         */
        post: operations["edit_staged_tags"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/files/{id}/audio": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** A file of the buffer zone, to listen to it. Supports range requests. */
        get: operations["get_staged_audio"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/files/{id}/choice": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Imports a file as one of its candidate releases, or with its own tags:
         *     heron-librarian works out its tags and path again. Operators and admins.
         */
        post: operations["choose"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/groups": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Starts an upload: its files are identified together, so the files of an
         *     album agree on their release. Operators and admins.
         */
        post: operations["create_group"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/import": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Imports files ready in the buffer zone: heron-librarian tags them and
         *     files them in the library as one batch, which can be undone. Files that
         *     are not ready are skipped (see the task's report). Operators and admins.
         */
        post: operations["import_files"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/uploads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Announces a file of an upload: it is created empty, and its content is
         *     sent in chunks. Refused when it is not audio, when it is larger than
         *     `max_upload_mb`, or when it would leave less than `min_free_mb` on the
         *     disk. Operators and admins.
         */
        post: operations["create_upload"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/staging/uploads/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * A file of an upload, with the bytes received so far: send the rest from
         *     there.
         */
        get: operations["get_upload"];
        /**
         * Appends a chunk (at most 16 MiB) to a file being uploaded, at `offset`,
         *     which must be the bytes received so far (409 otherwise: ask for the
         *     file and carry on from its `received`). With its last byte, the file is
         *     complete and heron-librarian identifies it. Operators and admins.
         */
        put: operations["send_chunk"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/status": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Whether the API, its database and the engine are up, and whether the
         *     server clock can be trusted. No sign-in needed.
         */
        get: operations["status"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Your API tokens (not revoked). The tokens themselves are never shown again. */
        get: operations["list_tokens"];
        put?: never;
        /** Creates an API token acting as you, with your role. */
        post: operations["create_token"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tokens/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Revokes one of your API tokens (admins: anyone's). */
        delete: operations["revoke_token"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Library tracks, searched, filtered and sorted, a page at a time. */
        get: operations["list_tracks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/categories": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Puts every selected track in categories, or takes them out. Filling a
         *     category from a label: select with `filters.labels_any`. Operators and
         *     admins.
         */
        post: operations["apply_categories"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/delete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Moves tracks to the trash: heron-librarian moves their files to
         *     `.heron-trash/` in the library, as one batch. They leave the library,
         *     rotation, their categories and the queue (a track on air plays to its
         *     end), and are purged after `trash_days`; until then they can be
         *     restored whole. Tracks whose file is missing can be deleted too.
         *     Operators and admins.
         */
        post: operations["delete_tracks"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/labels": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Adds labels to, and removes labels from, every selected track.
         *     Operators and admins.
         */
        post: operations["apply_labels"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/programming": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Sets the mood, sound code or tempo of every selected track. Operators
         *     and admins.
         */
        post: operations["apply_programming"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/tag-overrides": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Edits the tags of the selected tracks in Heron's database only, when
         *     the library setting `write_tags` is off: the files are not changed.
         *     The values are shown instead of the files' tags, and survive a rescan,
         *     until they are cleared. Operators and admins.
         */
        post: operations["override_tags"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/tag-overrides/clear": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Drops the tags kept in Heron's database for the selected tracks (at most
         *     10,000): they show their files' tags again. Works whatever `write_tags`
         *     says. Operators and admins.
         */
        post: operations["clear_tag_overrides"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/tags": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Writes tags into the files of the selected tracks (at most 10,000), as
         *     one batch that can be undone: queues a library task for
         *     heron-librarian. Files are not moved. Operators and admins.
         */
        post: operations["edit_tags"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * A track with its details, analysis, overrides, categories and
         *     identification.
         */
        get: operations["get_track"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /** Enables or disables a track. Operators and admins. */
        patch: operations["patch_track"];
        trace?: never;
    };
    "/api/v1/tracks/{id}/audio": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The track's audio file as it is in the library, for previews in the
         *     browser. Supports range requests.
         */
        get: operations["get_audio"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}/categories": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /** Replaces the track's categories. Operators and admins. */
        put: operations["put_categories"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}/overrides": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        /**
         * Replaces the track's overrides: cue points, Gap Killer, crossfade.
         *     Missing fields use the analysis and station settings. Queued entries of
         *     the track are updated within seconds. Operators and admins.
         */
        put: operations["put_overrides"];
        post?: never;
        /** Removes the track's overrides. Operators and admins. */
        delete: operations["delete_overrides"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}/review/apply": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Applies one of the track's candidate releases: heron-librarian writes
         *     its tags into the file and files it where the layout says, as a batch
         *     that can be undone. Operators and admins.
         */
        post: operations["apply_release"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}/review/keep": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Marks the track as reviewed: its file keeps its own tags. Operators and
         *     admins.
         */
        post: operations["keep_tags"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/tracks/{id}/waveform": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The track's waveform overview, computed with its analysis. 404 until
         *     the track is analyzed.
         */
        get: operations["get_waveform"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/users": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Every user. Admins only. */
        get: operations["list_users"];
        put?: never;
        /** Admins only. */
        post: operations["create_user"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/users/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Deletes a user with their sessions and tokens. Admins only. */
        delete: operations["delete_user"];
        options?: never;
        head?: never;
        /**
         * Changes a user's role, enables or disables them, or resets their
         *     password. Admins only.
         */
        patch: operations["update_user"];
        trace?: never;
    };
    "/api/v1/webhook-deliveries/{id}/retry": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Sends a delivered or failed delivery again, with every attempt again.
         *     Admins only.
         */
        post: operations["retry_delivery"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/webhooks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Webhooks, by name. Admins only. */
        get: operations["list_webhooks"];
        put?: never;
        /**
         * Adds a webhook. The answer holds its secret, shown only this once.
         *     Admins only.
         */
        post: operations["create_webhook"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/webhooks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_webhook"];
        /** Replaces a webhook; its secret is kept. Admins only. */
        put: operations["update_webhook"];
        post?: never;
        /** Deletes a webhook and its deliveries. Admins only. */
        delete: operations["delete_webhook"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/webhooks/{id}/deliveries": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** A webhook's deliveries, most recent first. Admins only. */
        get: operations["list_deliveries"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/webhooks/{id}/secret": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Gives a webhook a new secret, shown only this once; deliveries not sent
         *     yet are signed with it. Admins only.
         */
        post: operations["rotate_secret"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/v1/webhooks/{id}/test": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Sends a `ping` delivery now, whether the webhook is enabled or not.
         *     Admins only.
         */
        post: operations["test_webhook"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        AnalysisQueueStatus: {
            /** Format: int64 */
            failed: number;
            /** Format: int64 */
            pending: number;
            /** Format: int64 */
            running: number;
        };
        /**
         * @description Parameters of the import-time analysis (Gap Killer and crossfade point
         *     detection). Stored under the `analysis` settings name.
         *
         *     Levels are in dB **relative to the track's integrated loudness**, so the
         *     same thresholds work for quiet and loud masters. See `docs/analysis.md`.
         */
        AnalysisSettings: {
            /**
             * Format: int32
             * @description Bursts of sound this short, surrounded by silence, are ignored.
             * @default 10
             */
            blip_max_ms: number;
            /**
             * Format: double
             * @description Audio quieter than this at the end of a track is trimmed.
             * @default -24
             */
            end_threshold_db: number;
            /**
             * Format: int32
             * @description Longest distance between the crossfade point and the end of the track.
             * @default 2500
             */
            fade_max_ms: number;
            /**
             * Format: int32
             * @description Shortest distance between the crossfade point and the end of the track.
             * @default 150
             */
            fade_min_ms: number;
            /**
             * Format: double
             * @description The crossfade starts after the last moment the track is louder than this.
             * @default -15
             */
            fade_trigger_db: number;
            /**
             * Format: int32
             * @description Silence longer than this inside a track ends the track there.
             * @default 15000
             */
            max_internal_silence_ms: number;
            /**
             * Format: double
             * @description Absolute ceiling (dBFS) for what counts as silence: quiet but audible
             *     passages above it are never trimmed, whatever the track's loudness.
             * @default -50
             */
            silence_ceiling_dbfs: number;
            /**
             * Format: double
             * @description Audio quieter than this at the start of a track is trimmed.
             * @default -20
             */
            start_threshold_db: number;
        };
        ApiToken: {
            /** Format: int64 */
            created_at_unix_ms: number;
            /** Format: int64 */
            expires_at_unix_ms?: number | null;
            /** Format: int64 */
            id: number;
            /** Format: int64 */
            last_used_at_unix_ms?: number | null;
            name: string;
        };
        Artist: {
            /** Format: int64 */
            id: number;
            musicbrainz_id?: string | null;
            name: string;
            names: components["schemas"]["ArtistName"][];
            /** @description Separation treats these as the same artist. */
            similar: components["schemas"]["ArtistRef"][];
            /** Format: int64 */
            tracks: number;
        };
        ArtistName: {
            /** @description Moved to this artist by hand. */
            by_hand: boolean;
            /** Format: int64 */
            id: number;
            name: string;
        };
        ArtistPage: {
            artists: components["schemas"]["Artist"][];
            /** Format: int64 */
            total: number;
        };
        ArtistPatch: {
            /** @description `null` removes it. */
            musicbrainz_id?: string | null;
            /** @description The name shown. */
            name?: string | null;
        };
        ArtistRef: {
            /** Format: int64 */
            id: number;
            name: string;
        };
        ArtistSuggestion: {
            artist: components["schemas"]["ArtistRef"];
            /** Format: int64 */
            id: number;
            other: components["schemas"]["ArtistRef"];
            reason: string;
            /** @description `names` or `musicbrainz`. */
            source: string;
        };
        Batch: {
            /** Format: int64 */
            created_at_unix_ms: number;
            /**
             * Format: int64
             * @description Files moved or re-tagged.
             */
            files: number;
            /**
             * Format: int64
             * @description `None` while it runs, or when it was interrupted.
             */
            finished_at_unix_ms?: number | null;
            /** Format: int64 */
            id: number;
            kind: components["schemas"]["BatchKind"];
            /**
             * Format: int64
             * @description For an undo, the batch it undoes.
             */
            undoes?: number | null;
            /**
             * Format: int64
             * @description The undo of this batch.
             */
            undone_by?: number | null;
        };
        /** @enum {string} */
        BatchKind: "import" | "organize" | "undo" | "review" | "edit" | "delete" | "purge";
        /** @description What a batch of file operations did. */
        BatchReport: {
            /** Format: int64 */
            batch_id: number;
            /** Format: int64 */
            done: number;
            /** @description Operations not done, with the reason. */
            skipped: components["schemas"]["SkippedFile"][];
            /**
             * Format: int64
             * @description Files that needed no change (their tags already had the values).
             */
            unchanged?: number;
        };
        Blocked: {
            /** Format: int32 */
            album: number;
            /** Format: int32 */
            artist: number;
            /** Format: int32 */
            title: number;
            /** Format: int32 */
            track: number;
        };
        Buffer: {
            /**
             * Format: int64
             * @description Free space on the buffer folder's disk.
             */
            free_bytes?: number | null;
            /** @description Groups with files still to import or discard, newest first. */
            groups: components["schemas"]["StagedGroup"][];
            /** Format: int64 */
            max_upload_bytes: number;
            /**
             * Format: int64
             * @description Uploads are refused below this much free space.
             */
            min_free_bytes: number;
        };
        CategoriesChange: {
            /**
             * @description Categories every selected track is put in (with this weight, also
             *     for tracks already in them).
             */
            add?: components["schemas"]["CategoryShare"][];
            /** @description Categories every selected track is taken out of. */
            remove?: number[];
            selection: components["schemas"]["TrackSelection"];
        };
        Category: {
            /** @description `#rrggbb`. */
            color?: string | null;
            /**
             * @description Liners, power intros, voice tracks: the FX player's picker opens on
             *     these categories. Clocks still call them like any other.
             */
            fx_player: boolean;
            /** Format: int64 */
            id: number;
            name: string;
            /**
             * @description Windows this category overrides; the others follow the rotation
             *     settings (clock slots can override them again).
             */
            separation: components["schemas"]["SeparationOverrides"];
            /** Format: int64 */
            tracks: number;
            /**
             * Format: int64
             * @description What it holds (`GET /category-types`); none when unset.
             */
            type_id?: number | null;
            type_name?: string | null;
        };
        CategoryAssignment: {
            /** Format: int64 */
            category_id: number;
            /**
             * Format: double
             * @description Relative chance of being picked within the category, 1 by default.
             */
            weight?: number;
        };
        CategoryInput: {
            /** @description `#rrggbb`, or none. */
            color?: string | null;
            /** @description Not for the FX player by default. */
            fx_player?: boolean;
            name: string;
            /** @description None overridden by default. */
            separation?: components["schemas"]["SeparationOverrides"];
            /**
             * Format: int64
             * @description A category type; none by default.
             */
            type_id?: number | null;
        };
        CategoryShare: {
            /** Format: int64 */
            category_id: number;
            /**
             * Format: double
             * @description Relative chance of being picked within the category, 1 by default.
             */
            weight?: number;
        };
        CategoryType: {
            /**
             * Format: int64
             * @description Categories of this type.
             */
            categories: number;
            /** Format: int64 */
            id: number;
            name: string;
            /**
             * @description Never two tracks of this type in a row: rotation passes a slot
             *     calling it over right after one.
             */
            no_two_in_a_row: boolean;
            /**
             * @description Its tracks send metadata to the stream servers. Off (jingles, ads):
             *     the stream and the public now-playing keep the last song's, and
             *     webhooks send them only when they include the FX player.
             */
            sends_metadata: boolean;
        };
        CategoryTypeInput: {
            name: string;
            /** @description Kept as it is when missing (off for a new type). */
            no_two_in_a_row?: boolean | null;
            /** @description Kept as it is when missing (on for a new type). */
            sends_metadata?: boolean | null;
        };
        /** @description dBFS, −120 for silence. */
        ChannelLevels: {
            /** Format: float */
            peak_left: number;
            /** Format: float */
            peak_right: number;
            /** Format: float */
            rms_left: number;
            /** Format: float */
            rms_right: number;
        };
        ChoiceRequest: {
            /** @enum {string} */
            kind: "release";
            recording_id: string;
            release_id: string;
        } | {
            /** @enum {string} */
            kind: "own";
        };
        ClockInput: {
            /** @description Off by default: the clock starts over at each hour. */
            looping?: boolean;
            name: string;
            slots: components["schemas"]["ClockSlot"][];
        };
        ClockPreview: {
            /**
             * Format: int64
             * @description Expected length of the whole clock.
             */
            duration_ms: number;
            /**
             * Format: double
             * @description How many times the clock runs in an hour: more than 1 when it is
             *     shorter than the hour; under 1 for a looping clock longer than it.
             */
            runs_per_hour: number;
            slots: components["schemas"]["SlotPreview"][];
        };
        /** @description How a slot calls its category. */
        ClockSlot: {
            /** Format: int64 */
            category_id: number;
            /** @description Off, the slot skips the separation rules (restrictions still apply). */
            enforce_rules?: boolean;
            method?: components["schemas"]["Method"];
            /** @description Windows this slot overrides, over its category's. */
            separation?: components["schemas"]["SeparationOverrides"];
        };
        ClockStatus: {
            /**
             * Format: double
             * @description The kernel's estimate of the clock's error, in ms.
             */
            estimated_error_ms?: number | null;
            /**
             * Format: double
             * @description The offset the kernel is correcting, in ms.
             */
            offset_ms?: number | null;
            /** @description `None` where the kernel's state cannot be read (not Linux). */
            synchronized?: boolean | null;
            /**
             * @description Why the clock cannot be trusted, for people. `None` when it is fine
             *     or unknown.
             */
            warning?: string | null;
        };
        ClockView: {
            /** Format: int64 */
            id: number;
            /** @description Carries on from its last slot across hours instead of starting over. */
            looping: boolean;
            name: string;
            /**
             * Format: int64
             * @description Hours of the week it is scheduled at.
             */
            scheduled_hours: number;
            slots: components["schemas"]["ClockSlot"][];
        };
        CreatedToken: {
            /** Format: int64 */
            id: number;
            name: string;
            /** @description Send it as `Authorization: Bearer <token>`. Shown only once. */
            token: string;
        };
        /** @description A new webhook, with its secret: the only time it is shown. */
        CreatedWebhook: {
            secret: string;
            webhook: components["schemas"]["WebhookView"];
        };
        /**
         * @description Where the next track starts.
         * @enum {string}
         */
        CrossfadeMode: "auto" | "fixed";
        /**
         * @description Per-track crossfade overrides (`track_overrides.crossfade`). Missing fields
         *     use the station settings. The fade-out, mode and fixed time apply to the
         *     transition out of the track; the fade-in to the transition into it.
         */
        CrossfadeOverrides: {
            /**
             * @description `false` plays the transition out of this track back to back, without overlap.
             * @default null
             */
            crossfade: boolean | null;
            /** @default null */
            fade_in: components["schemas"]["Fade"] | null;
            /** @default null */
            fade_out: components["schemas"]["Fade"] | null;
            /**
             * Format: int32
             * @default null
             */
            fixed_ms: number | null;
            /** @default null */
            mode: components["schemas"]["CrossfadeMode"] | null;
        };
        /**
         * @description Station-wide crossfade settings, modeled on SAM Broadcaster's
         *     "Cross-Fading" screen. Stored under `playout` in the `settings` table.
         */
        CrossfadeSettings: {
            /**
             * @description Fade-in of the incoming track.
             * @default {
             *       "curve": "linear",
             *       "enabled": false,
             *       "level_pct": 0,
             *       "time_ms": 1000
             *     }
             */
            fade_in: components["schemas"]["Fade"];
            /**
             * @description Fade-out of the outgoing track, from the moment the next one starts.
             * @default {
             *       "curve": "linear",
             *       "enabled": true,
             *       "level_pct": 0,
             *       "time_ms": 2500
             *     }
             */
            fade_out: components["schemas"]["Fade"];
            /**
             * Format: int32
             * @description With [`CrossfadeMode::Fixed`], how long before the cue-out the next track starts.
             * @default 2000
             */
            fixed_ms: number;
            /**
             * Format: int32
             * @description Tracks this long or shorter (jingles, IDs) are never crossfaded: they
             *     start right after the previous track and the next starts right after them.
             * @default 20000
             */
            min_track_ms: number;
            /** @default auto */
            mode: components["schemas"]["CrossfadeMode"];
            /**
             * Format: int32
             * @description Fade-out of the current track when it is skipped.
             * @default 1000
             */
            skip_fade_ms: number;
        };
        /** @enum {string} */
        DeckName: "a" | "b" | "fx";
        DeckPosition: {
            deck: components["schemas"]["DeckName"];
            /** Format: int64 */
            length_ms: number;
            /** Format: int64 */
            position_ms: number;
        };
        DeckState: {
            deck: components["schemas"]["DeckName"];
            entry?: components["schemas"]["QueueEntry"] | null;
            /** Format: int64 */
            length_ms: number;
            /** @description The track listeners hear as "now playing". */
            on_air: boolean;
            /**
             * Format: int64
             * @description From the cue-in.
             */
            position_ms: number;
        };
        /** @enum {string} */
        DeliveryStatus: "pending" | "delivered" | "failed";
        DeliverySummary: {
            /** Format: int64 */
            created_at_unix_ms: number;
            /** Format: int32 */
            last_status_code?: number | null;
            status: components["schemas"]["DeliveryStatus"];
        };
        DeliveryView: {
            /** Format: int32 */
            attempts: number;
            /** @description What was sent. */
            body: string;
            /** Format: int64 */
            created_at_unix_ms: number;
            /** Format: int64 */
            delivered_at_unix_ms?: number | null;
            event: components["schemas"]["WebhookEvent"];
            /** Format: int64 */
            id: number;
            /** Format: int64 */
            last_attempt_at_unix_ms?: number | null;
            /** @description Why the last attempt failed. */
            last_error?: string | null;
            /** @description The start of the receiver's last answer. */
            last_response?: string | null;
            /**
             * Format: int32
             * @description The receiver's HTTP status.
             */
            last_status_code?: number | null;
            /**
             * Format: int64
             * @description While pending.
             */
            next_attempt_at_unix_ms?: number | null;
            status: components["schemas"]["DeliveryStatus"];
            /** Format: int64 */
            webhook_id: number;
        };
        /** @description What a MusicBrainz details lookup did. */
        DetailsReport: {
            /**
             * Format: int64
             * @description Lookups that failed (tried again next time).
             */
            failed: number;
            /**
             * Format: int64
             * @description Releases looked up (one request each).
             */
            releases: number;
            /**
             * Format: int64
             * @description Tracks whose details were saved.
             */
            tracks: number;
        };
        DetailsRequest: {
            /**
             * @description Look up every identified track again, not only those never looked up.
             * @default false
             */
            all: boolean;
        };
        Discarded: {
            /**
             * Format: int64
             * @description Files removed from the buffer zone.
             */
            discarded: number;
        };
        /** @description How the engine will play the track: analysis and overrides combined. */
        EffectivePlayout: {
            /** Format: int32 */
            cue_in_ms: number;
            /** Format: int32 */
            cue_out_ms: number;
            /**
             * Format: double
             * @description Loudness normalization gain with the current playout settings.
             */
            gain_db: number;
            /** Format: int32 */
            mix_point_ms?: number | null;
        };
        /** @enum {string} */
        EncoderKind: "mp3" | "aac" | "opus" | "vorbis" | "external_ffmpeg";
        EncoderProfileInput: {
            /** Format: int32 */
            bitrate_kbps?: number | null;
            /** Format: int32 */
            channels: number;
            encoder: components["schemas"]["EncoderKind"];
            name: string;
            options?: Record<string, never>;
            /** Format: int32 */
            sample_rate: number;
        };
        EncoderProfileView: {
            /** Format: int32 */
            bitrate_kbps?: number | null;
            /** Format: int32 */
            channels: number;
            encoder: components["schemas"]["EncoderKind"];
            /** Format: int64 */
            id: number;
            name: string;
            /** @description Encoder-specific options, e.g. MP3: `{"mode": "cbr", "quality": 3}`. */
            options: Record<string, never>;
            /**
             * Format: int32
             * @description Outputs using this profile.
             */
            outputs: number;
            /** Format: int32 */
            sample_rate: number;
        };
        /** @enum {string} */
        EndReason: "finished" | "skipped" | "stopped" | "replaced";
        /** @description Whether the engine is reachable and runs the stored configuration. */
        EngineStatus: {
            /** @description Playout settings and outputs. */
            config: components["schemas"]["SyncStatus"];
            connected: boolean;
            /** Format: int64 */
            started_at_unix_ms?: number | null;
        };
        EnqueueRequest: {
            /**
             * Format: int32
             * @description Where among the waiting tracks, 0 being next. At the end when missing.
             */
            position?: number | null;
            /** @description Library tracks, played in this order. */
            track_ids: number[];
        };
        EnqueueResponse: {
            entry_ids: number[];
        };
        Era: {
            /**
             * Format: int32
             * @description First year, open-ended when missing.
             */
            from_year?: number | null;
            name: string;
            /**
             * Format: int32
             * @description Last year, open-ended when missing.
             */
            to_year?: number | null;
        };
        /** @description Body of every error response. */
        ErrorBody: {
            /**
             * @description Machine-readable code: `invalid_request`, `unauthorized`, `forbidden`,
             *     `not_found`, `conflict`, `invalid`, `too_many_requests`,
             *     `engine_unavailable` or `internal`.
             */
            error: string;
            /** @description For people. */
            message: string;
        };
        /** @description Why a track was picked: the selection log. */
        Explanation: {
            /**
             * Format: int64
             * @description When the track is planned to start (Unix ms).
             */
            at_unix_ms: number;
            /**
             * @description Tracks of the slot's category held back by each separation rule
             *     with the rules as configured (the first rule that applies counts).
             */
            blocked: components["schemas"]["Blocked"];
            /**
             * Format: int32
             * @description Tracks the method chose among.
             */
            candidates: number;
            category_id?: components["schemas"]["u64"] | null;
            category_name?: string | null;
            clock_id?: components["schemas"]["u64"] | null;
            clock_name?: string | null;
            enforce_rules: boolean;
            /**
             * Format: int64
             * @description When the picked track last played; missing if never.
             */
            last_played_unix_ms?: number | null;
            /** @description Applied: least recently played at step 10. */
            method: components["schemas"]["Method"];
            /**
             * Format: int32
             * @description Tracks in the slot's category.
             */
            pool: number;
            /**
             * Format: int32
             * @description Of them, forbidden by restrictions.
             */
            restricted: number;
            /** @description Slots passed over because nothing could be picked, even relaxed. */
            skipped_slots: components["schemas"]["SkippedSlot"][];
            /**
             * Format: int32
             * @description Index of the slot in the clock, from 0.
             */
            slot?: number | null;
            slot_category_id?: components["schemas"]["u64"] | null;
            /**
             * Format: int32
             * @description Relaxation step, 0 (none) to 10.
             */
            step: number;
            step_description: string;
            /**
             * Format: double
             * @description Weight of the picked track in its category.
             */
            weight: number;
            /** @description Separation windows applied at this step. */
            windows: components["schemas"]["Windows"];
        };
        /** @description One side of a crossfade. */
        Fade: {
            /** @default linear */
            curve: components["schemas"]["FadeCurve"];
            /** @default true */
            enabled: boolean;
            /**
             * Format: int32
             * @description Level the fade-out goes down to, or the fade-in starts from, in percent.
             * @default 0
             */
            level_pct: number;
            /**
             * Format: int32
             * @default 2500
             */
            time_ms: number;
        };
        /**
         * @description Shape of a fade.
         * @enum {string}
         */
        FadeCurve: "linear" | "s_curve" | "logarithmic" | "exponential";
        FileSelection: {
            file_ids: number[];
        };
        /** @enum {string} */
        GroupSource: "upload" | "incoming";
        /** @description An hour of the day, over the days of a period. */
        HourOfDay: {
            /**
             * Format: double
             * @description Mean of the listeners over the minutes measured.
             */
            average: number;
            /**
             * Format: int32
             * @description Days with a minute measured in that hour.
             */
            days: number;
            /**
             * Format: int32
             * @description 0 to 23, station time.
             */
            hour: number;
            /**
             * Format: double
             * @description Listening hours, on average per day.
             */
            listening_hours: number;
            /**
             * Format: int32
             * @description The highest peak.
             */
            peak: number;
        };
        ImportApplyRequest: {
            categories?: components["schemas"]["ImportCategoryChoice"][];
            problems?: components["schemas"]["ImportProblemChoice"][];
            /** @description The values to set on each track, conflicts settled. */
            rows: components["schemas"]["ImportApplyRow"][];
        };
        ImportApplyRow: {
            /** Format: int64 */
            track_id: number;
            values: components["schemas"]["ImportValues"];
        };
        /** @description A category of the export. */
        ImportCategory: {
            /** @description The other software never schedules it. */
            can_schedule: boolean;
            /**
             * Format: int64
             * @description The Heron category of the same name, if there is one.
             */
            heron_category_id?: number | null;
            /** Format: int64 */
            id: number;
            is_jingle: boolean;
            name: string;
            /** Format: int64 */
            parent_id?: number | null;
            /**
             * Format: int64
             * @description Rows in it.
             */
            rows: number;
        };
        /** @description What an export's category becomes in Heron. */
        ImportCategoryChoice: {
            /** @description Its tracks are disabled: rotation never picks them. */
            disable_tracks?: boolean;
            /** @description Offered by the FX player (set when the category is created). */
            fx_player?: boolean;
            /**
             * Format: int64
             * @description The export's category.
             */
            id: number;
            /**
             * @description The Heron category its tracks go in, created if missing; none leaves
             *     them out of any category.
             */
            name?: string | null;
            /**
             * Format: int64
             * @description Its type (`GET /category-types`), set when the category is created.
             */
            type_id?: number | null;
        };
        /**
         * @description A value Heron may already have, set in Heron: a conflict when the
         *     import's differs.
         * @enum {string}
         */
        ImportField: "title" | "artist" | "year" | "fade_in" | "fade_out" | "intro" | "intro2" | "hook_in" | "hook_out" | "next_soft" | "next_hard";
        ImportPreview: {
            categories: components["schemas"]["ImportCategory"][];
            problems: components["schemas"]["ImportProblem"][];
            rows: components["schemas"]["ImportRow"][];
        };
        ImportPreviewRequest: {
            /** @description The export's rows, in order. */
            rows: components["schemas"]["ImportSourceRow"][];
        };
        /** @description A problem of the export. */
        ImportProblem: {
            /** Format: int64 */
            id: number;
            name: string;
            /** Format: int64 */
            rows: number;
        };
        /** @enum {string} */
        ImportProblemAction: "ignore" | "label" | "label_and_disable";
        /** @description What an export's problem becomes in Heron. */
        ImportProblemChoice: {
            action: components["schemas"]["ImportProblemAction"];
            /** Format: int64 */
            id: number;
            /** @description The label's name, created if missing. */
            label?: string | null;
        };
        ImportRow: {
            /** @description Fields set in Heron that the import would change. */
            conflicts: components["schemas"]["ImportField"][];
            /** @description What Heron has set itself, for the same fields. */
            current: components["schemas"]["ImportValues"];
            filename: string;
            /** @description The lengths differ by more than 2 s: probably another edit. */
            length_mismatch: boolean;
            /** Format: int32 */
            length_ms?: number | null;
            /**
             * Format: int32
             * @description 1 for the first row of the export.
             */
            line: number;
            /** @description The library file it matched. */
            path?: string | null;
            /**
             * Format: int32
             * @description The export's length of the file.
             */
            source_length_ms?: number | null;
            status: components["schemas"]["ImportRowStatus"];
            /** Format: int64 */
            track_id?: number | null;
            /** @description What the import would set. */
            values: components["schemas"]["ImportValues"];
            /** @description Values of the export left out, and why. */
            warnings: string[];
        };
        /** @enum {string} */
        ImportRowStatus: "matched" | "not_found" | "duplicate_in_export" | "duplicate_in_library" | "invalid";
        /**
         * @description One audio file of the other software's export. Times are in seconds, on
         *     the timeline of the file the software measured them on; 0 means "not
         *     set". Unknown fields are ignored.
         */
        ImportSourceRow: {
            /** @default null */
            artist: string | null;
            /**
             * Format: int32
             * @description 0: the software never schedules this category.
             * @default null
             */
            category_can_schedule: number | null;
            /**
             * Format: int64
             * @default null
             */
            category_id: number | null;
            /**
             * Format: int32
             * @description 1: a category of jingles.
             * @default null
             */
            category_is_jingle: number | null;
            /** @default null */
            category_name: string | null;
            /**
             * Format: int64
             * @default null
             */
            category_parent_id: number | null;
            /**
             * @description When the track was added: `2022-03-11 09:35:38`.
             * @default null
             */
            created_at: string | null;
            /**
             * Format: double
             * @description Where the file was cut; the other times are shifted by it.
             * @default null
             */
            cue_in: number | null;
            /**
             * Format: double
             * @description Length of the fade-in.
             * @default null
             */
            fade_in: number | null;
            /**
             * Format: double
             * @description Length of the fade-out.
             * @default null
             */
            fade_out: number | null;
            /**
             * @description The file's name; a folder in front of it is ignored.
             * @default
             */
            filename: string;
            /**
             * Format: double
             * @description Length of the file the export describes, to check it is the same.
             * @default null
             */
            final_duration: number | null;
            /**
             * Format: double
             * @default null
             */
            hook_in: number | null;
            /**
             * Format: double
             * @default null
             */
            hook_out: number | null;
            /**
             * Format: double
             * @default null
             */
            intro: number | null;
            /**
             * Format: double
             * @default null
             */
            intro2: number | null;
            /**
             * Format: int64
             * @description 0: no problem.
             * @default null
             */
            problem_id: number | null;
            /** @default null */
            problem_name: string | null;
            /**
             * Format: double
             * @default null
             */
            start_next_hard: number | null;
            /**
             * Format: double
             * @default null
             */
            start_next_soft: number | null;
            /** @default null */
            title: string | null;
            /**
             * @description `1997`, as text or as a number.
             * @default null
             */
            year: string | null;
        };
        ImportSummary: {
            categories_created: string[];
            /** Format: int64 */
            disabled: number;
            labels_created: string[];
            /** Format: int64 */
            tracks: number;
        };
        /**
         * @description Values an import sets on a track; a missing value leaves Heron's alone.
         *     Times in ms on the track's file.
         */
        ImportValues: {
            /**
             * @description When the track was added: `2022-03-11 09:35:38`, station time.
             * @default null
             */
            added_at: string | null;
            /** @default null */
            artist: string | null;
            /**
             * Format: int64
             * @description The export's category (see `ImportCategoryChoice`).
             * @default null
             */
            category: number | null;
            /**
             * Format: int32
             * @default null
             */
            fade_in_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            fade_out_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            hook_in_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            hook_out_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            intro2_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            intro_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            next_hard_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            next_soft_ms: number | null;
            /**
             * Format: int64
             * @description The export's problem (see `ImportProblemChoice`).
             * @default null
             */
            problem: number | null;
            /** @default null */
            title: string | null;
            /** @default null */
            year: string | null;
        };
        Label: {
            /** @description `#rrggbb`. */
            color?: string | null;
            /** Format: int64 */
            id: number;
            name: string;
            /** Format: int64 */
            tracks: number;
        };
        LabelInput: {
            /** @description `#rrggbb`, or none. */
            color?: string | null;
            /** @description Unique, ignoring case. */
            name: string;
        };
        LabelMerge: {
            /**
             * Format: int64
             * @description The label that stays.
             */
            into_id: number;
        };
        LabelRef: {
            color?: string | null;
            /** Format: int64 */
            id: number;
            name: string;
        };
        LabelsChange: {
            /** @description Labels added to every selected track. */
            add?: number[];
            /** @description Labels removed from every selected track. */
            remove?: number[];
            selection: components["schemas"]["TrackSelection"];
        };
        Levels: {
            deck_a: components["schemas"]["ChannelLevels"];
            deck_b: components["schemas"]["ChannelLevels"];
            fx: components["schemas"]["ChannelLevels"];
            master: components["schemas"]["ChannelLevels"];
        };
        /** @description Changes to the library settings: missing fields are kept. */
        LibrarySettingsPatch: {
            /** @description The station's AcoustID application key; `null` removes it. */
            acoustid_key?: string | null;
            embed_cover?: boolean | null;
            /** @description See `docs/library.md#layout`. */
            layout?: string | null;
            /**
             * Format: int64
             * @description 1 or more.
             */
            max_upload_mb?: number | null;
            /** Format: int64 */
            min_free_mb?: number | null;
            /**
             * Format: double
             * @description Lowest AcoustID score (0 to 1) accepted as a match.
             */
            min_score?: number | null;
            save_cover?: boolean | null;
            /**
             * Format: int32
             * @description 1 to 3650.
             */
            trash_days?: number | null;
            /**
             * @description Write tags edited from the browser into the files (true), or keep
             *     them in Heron's database (false).
             */
            write_tags?: boolean | null;
        };
        /** @description Library settings as shown: the AcoustID key is never sent back. */
        LibrarySettingsView: {
            /** @description Whether the station has set its AcoustID key (identification is off without one). */
            acoustid_key_set: boolean;
            embed_cover: boolean;
            layout: string;
            /**
             * Format: int64
             * @description Largest file accepted by upload, in MiB.
             */
            max_upload_mb: number;
            /**
             * Format: int64
             * @description Uploads are refused when they would leave less free space on the
             *     buffer zone's disk, in MiB.
             */
            min_free_mb: number;
            /** Format: double */
            min_score: number;
            save_cover: boolean;
            /**
             * Format: int32
             * @description Days deleted tracks stay in the trash.
             */
            trash_days: number;
            /**
             * @description Tags edited from the browser are written into the files; off, they
             *     are kept in Heron's database and the files are left alone.
             */
            write_tags: boolean;
        };
        /** @description Library and analysis progress, served by `/api/v1/library/status`. */
        LibraryStatus: {
            analysis_queue: components["schemas"]["AnalysisQueueStatus"];
            /**
             * Format: int64
             * @description Present tracks with an up-to-date analysis.
             */
            analyzed: number;
            last_scan?: components["schemas"]["ScanReport"] | null;
            /** Format: int64 */
            missing: number;
            /**
             * Format: int64
             * @description Analyzed tracks with nothing audible.
             */
            silent: number;
            /**
             * Format: int64
             * @description Tracks whose file is present.
             */
            tracks: number;
        };
        LibraryTask: {
            /** Format: int64 */
            created_at_unix_ms: number;
            details?: components["schemas"]["DetailsReport"] | null;
            /** @description Why it failed. */
            error?: string | null;
            /** Format: int64 */
            finished_at_unix_ms?: number | null;
            /** Format: int64 */
            id: number;
            plan?: components["schemas"]["OrganizePlan"] | null;
            purge?: components["schemas"]["PurgeReport"] | null;
            relations?: components["schemas"]["RelationsReport"] | null;
            report?: components["schemas"]["BatchReport"] | null;
            request: components["schemas"]["TaskRequest"];
            requested_by?: string | null;
            /** Format: int64 */
            started_at_unix_ms?: number | null;
            status: components["schemas"]["TaskStatus"];
        };
        ListenersNow: {
            /** @description Each enabled output read at the last minute. */
            outputs: components["schemas"]["OutputListeners"][];
            /**
             * Format: int32
             * @description The outputs' listeners added up; missing before the first reading or
             *     when no server could be read.
             */
            total?: number | null;
        };
        /** @description A message on the live WebSocket. */
        LiveMessage: {
            player?: components["schemas"]["PlayerState"] | null;
            /** @enum {string} */
            type: "state";
        } | {
            /** Format: int64 */
            at_unix_ms: number;
            decks: components["schemas"]["DeckPosition"][];
            /** @enum {string} */
            type: "positions";
        } | {
            levels: components["schemas"]["Levels"];
            /** @enum {string} */
            type: "levels";
        } | {
            event: components["schemas"]["TrackEvent"];
            /** @enum {string} */
            type: "track";
        };
        LoginRequest: {
            password: string;
            username: string;
        };
        /** @description Loudness normalization from the analyzed integrated loudness (EBU R128). */
        LoudnessSettings: {
            /** @default true */
            enabled: boolean;
            /**
             * Format: double
             * @description Quiet tracks are boosted by at most this much.
             * @default 12
             */
            max_gain_db: number;
            /**
             * Format: double
             * @description Boosts never push the track's true peak above this. There is no
             *     limiter before M10, so a boost that clips would distort.
             * @default -1
             */
            peak_ceiling_dbtp: number;
            /**
             * Format: double
             * @default -16
             */
            target_lufs: number;
        };
        MergeRequest: {
            /**
             * Format: int64
             * @description The artist merged into this one, then deleted.
             */
            artist_id: number;
        };
        /**
         * @description How a clock slot picks among the tracks its category allows.
         * @enum {string}
         */
        Method: "least_recently_played" | "random" | "weighted";
        MinuteSample: {
            /**
             * Format: int64
             * @description The start of the minute.
             */
            at_unix_ms: number;
            /** Format: int32 */
            listeners: number;
        };
        /** @enum {string} */
        Mode: "allow" | "ban";
        MoveName: {
            /**
             * Format: int64
             * @description The artist the name now belongs to; missing to make it an artist
             *     of its own.
             */
            artist_id?: number | null;
        };
        MoveRequest: {
            /**
             * Format: int32
             * @description New place among the waiting tracks, 0 being next.
             */
            position: number;
        };
        NewGroup: {
            /** @description What the files are, for people (`Album 2024`, the dropped folder). */
            name: string;
        };
        NewToken: {
            /**
             * Format: int32
             * @description Never expires when missing.
             */
            expires_in_days?: number | null;
            /** @description What the token is for. */
            name: string;
        };
        NewUpload: {
            /** Format: int64 */
            group_id: number;
            /** @description The file's path in what was dropped (`Album/01 Song.flac`). */
            name: string;
            /**
             * Format: int64
             * @description Size in bytes.
             */
            size: number;
        };
        NewUser: {
            /** @description At least 8 characters. */
            password: string;
            role: components["schemas"]["Role"];
            username: string;
        };
        NowPlaying: {
            now?: components["schemas"]["PublicTrack"] | null;
            /**
             * Format: int64
             * @description To tell how far the track has gone since.
             */
            server_time_unix_ms: number;
        };
        /** @description The dry run of an organization. */
        OrganizePlan: {
            moves: components["schemas"]["PlannedMove"][];
            /**
             * Format: int64
             * @description Tracks already where the layout puts them.
             */
            unchanged: number;
            /** @description Tracks whose file cannot be read (left where they are). */
            unreadable: string[];
        };
        /** @enum {string} */
        OutputConnection: "stopped" | "connecting" | "connected" | "reconnecting";
        OutputInput: {
            /** @description Sink settings. Credentials left out keep their stored value. */
            config: Record<string, never>;
            enabled?: boolean;
            /** Format: int64 */
            encoder_profile_id: number;
            kind: components["schemas"]["OutputKind"];
            metadata_enabled?: boolean;
            name: string;
            statistics?: components["schemas"]["OutputStatisticsInput"] | null;
        };
        /** @enum {string} */
        OutputKind: "icecast" | "shoutcast_v1" | "shoutcast_v2" | "hls" | "file" | "pipe";
        /** @description An output's listeners at the last minute. */
        OutputListeners: {
            /** @description Why the server, or its listener list, could not be read. */
            error?: string | null;
            /**
             * Format: int32
             * @description Missing when its server could not be read.
             */
            listeners?: number | null;
            name: string;
            /** Format: int64 */
            output_id: number;
            /** Format: int64 */
            sampled_at_unix_ms: number;
            /** @description Whether the listener list was read: sessions are up to date. */
            sessions: boolean;
        };
        /** @description Replaces the statistics settings of an output. */
        OutputStatisticsInput: {
            /** @description Its password: left out, the stored one is kept; `null` removes it. */
            admin_password?: string | null;
            /** @description The server's admin user; `admin` when missing. */
            admin_username?: string | null;
        };
        /** @description What listener statistics read the stream server with. */
        OutputStatisticsView: {
            /**
             * @description Whether its password is set (never sent back): without it, only
             *     concurrent listeners are counted.
             */
            admin_password_set: boolean;
            /** @description The server's admin user (`admin` when missing). */
            admin_username?: string | null;
        };
        /** @description The live state of an output, as the engine reports it. */
        OutputStatus: {
            /** Format: int64 */
            bytes_sent: number;
            /** Format: int64 */
            connected_since_unix_ms?: number | null;
            /** Format: int64 */
            dropped_chunks: number;
            enabled: boolean;
            /** @description Why the last connection failed, while reconnecting. */
            error?: string | null;
            /** Format: int64 */
            id: number;
            name: string;
            state: components["schemas"]["OutputConnection"];
        };
        OutputView: {
            /** @description Sink settings (Icecast: host, port, mount…), without credentials. */
            config: Record<string, never>;
            enabled: boolean;
            /** Format: int64 */
            encoder_profile_id: number;
            /** Format: int64 */
            id: number;
            kind: components["schemas"]["OutputKind"];
            /** @description Where listeners can play the stream (Icecast). */
            listen_url?: string | null;
            metadata_enabled: boolean;
            name: string;
            /** @description Credentials set in the settings (e.g. `password`), never sent back. */
            secrets_set: string[];
            statistics: components["schemas"]["OutputStatisticsView"];
            status?: components["schemas"]["OutputStatus"] | null;
        };
        PasswordChange: {
            current_password: string;
            new_password: string;
        };
        /** @description The totals of a period. */
        PeriodTotals: {
            /**
             * Format: double
             * @description Mean of the listeners over the minutes measured.
             */
            average: number;
            /** Format: double */
            average_session_s?: number | null;
            /** @description The first day of the period (`YYYY-MM-DD`). */
            day: string;
            /**
             * Format: int32
             * @description Days with a minute measured.
             */
            days: number;
            /**
             * Format: double
             * @description Distinct listeners; for weeks and months, the average per day.
             */
            distinct_listeners?: number | null;
            /**
             * Format: int32
             * @description The hour (0 to 23), with `group=hour`.
             */
            hour?: number | null;
            /**
             * Format: int32
             * @description Distinct listeners of each day, added up.
             */
            listener_days?: number | null;
            /**
             * Format: double
             * @description Total listening hours.
             */
            listening_hours: number;
            /**
             * Format: int32
             * @description Minutes measured.
             */
            minutes: number;
            /** Format: int32 */
            peak: number;
            /**
             * Format: int32
             * @description Sessions of the minimum length started in the period; missing
             *     without listener lists.
             */
            sessions?: number | null;
        };
        PlannedMove: {
            /** @description Relative to the library root. */
            from: string;
            to: string;
            /** Format: int64 */
            track_id: number;
        };
        /** @description One play. */
        Play: {
            album?: string | null;
            artist?: string | null;
            deck: components["schemas"]["DeckName"];
            end_reason?: components["schemas"]["EndReason"] | null;
            /**
             * Format: int64
             * @description Missing while the track plays.
             */
            ended_at_unix_ms?: number | null;
            /** Format: int64 */
            id: number;
            /** Format: int32 */
            played_ms?: number | null;
            source: components["schemas"]["PlaySource"];
            /** Format: int64 */
            started_at_unix_ms: number;
            title?: string | null;
            /**
             * Format: int64
             * @description Missing for files outside the library, and tracks deleted since.
             */
            track_id?: number | null;
        };
        PlayFxRequest: {
            /** Format: int64 */
            track_id: number;
        };
        PlayFxResponse: {
            /** Format: int64 */
            entry_id: number;
        };
        /**
         * @description Why a track is played (`play_history.source`).
         * @enum {string}
         */
        PlaySource: "rotation" | "request" | "fallback" | "manual";
        PlayerListening: {
            /** Format: double */
            listening_hours: number;
            /**
             * Format: int64
             * @description Sessions of the minimum length.
             */
            sessions: number;
            /** @description As the player sends it. */
            user_agent: string;
        };
        /** @description Decks, queue and outputs of the engine. */
        PlayerState: {
            cued?: components["schemas"]["QueueEntry"] | null;
            decks: components["schemas"]["DeckState"][];
            /** Format: int64 */
            engine_started_at_unix_ms: number;
            outputs: components["schemas"]["OutputStatus"][];
            paused: boolean;
            /** @description After the cued track. */
            queue: components["schemas"]["QueueEntry"][];
            /**
             * Format: int64
             * @description Audio rendered since the engine started.
             */
            rendered_ms: number;
            /** Format: int64 */
            underruns: number;
        };
        /**
         * @description Playout settings, stored under `playout` in the `settings` table and
         *     pushed to the engine.
         */
        PlayoutSettings: {
            /**
             * @default {
             *       "fade_in": {
             *         "curve": "linear",
             *         "enabled": false,
             *         "level_pct": 0,
             *         "time_ms": 1000
             *       },
             *       "fade_out": {
             *         "curve": "linear",
             *         "enabled": true,
             *         "level_pct": 0,
             *         "time_ms": 2500
             *       },
             *       "fixed_ms": 2000,
             *       "min_track_ms": 20000,
             *       "mode": "auto",
             *       "skip_fade_ms": 1000
             *     }
             */
            crossfade: components["schemas"]["CrossfadeSettings"];
            /**
             * @default {
             *       "enabled": true,
             *       "max_gain_db": 12,
             *       "peak_ceiling_dbtp": -1,
             *       "target_lufs": -16
             *     }
             */
            loudness: components["schemas"]["LoudnessSettings"];
            /**
             * @default {
             *       "enabled": true,
             *       "timeout_ms": 20000
             *     }
             */
            silence_watchdog: components["schemas"]["SilenceWatchdogSettings"];
        };
        /** @description What to change; what is left out stays as it is. */
        Preferences: {
            /** @description Use the whole width of the screen instead of a centred column. */
            full_width?: boolean | null;
            /**
             * @description A language tag (`en`, `fr`…) or `system`. The API does not know which
             *     languages the web UI has: it falls back on the browser's for one it
             *     lacks.
             */
            language?: string | null;
            theme?: components["schemas"]["Theme"] | null;
        };
        Preparation: {
            /**
             * @description Categories the files are put in (with this weight, also for files
             *     already in them).
             */
            add_categories?: components["schemas"]["CategoryShare"][];
            add_labels?: number[];
            file_ids: number[];
            /** @description A value of the mood scale, or `null` to clear it. Missing: kept. */
            mood?: string | null;
            /** @description Categories the files are taken out of. */
            remove_categories?: number[];
            remove_labels?: number[];
            /** @description A value of the sound code scale, or `null` to clear it. */
            sound_code?: string | null;
        };
        Prepared: {
            /**
             * Format: int64
             * @description Files prepared.
             */
            files: number;
        };
        PreviewRequest: {
            /** @description Whether the clock loops across hours. */
            looping?: boolean;
            /** @description The slots as edited, saved or not. */
            slots: components["schemas"]["ClockSlot"][];
        };
        ProgrammingChange: {
            /**
             * Format: double
             * @description The tempo, replacing the measured one; `null` goes back to it.
             */
            bpm?: number | null;
            /** @description A value of the mood scale, or `null` to clear it. Missing: kept. */
            mood?: string | null;
            selection: components["schemas"]["TrackSelection"];
            /** @description A value of the sound code scale, or `null` to clear it. */
            sound_code?: string | null;
        };
        /** @description What music programmers sort and separate tracks by. */
        ProgrammingSettings: {
            /**
             * @description Eras by year of first release, the first matching one applies.
             *     Empty: decades (`70s`, `80s`, `2000s`).
             * @default []
             */
            eras: components["schemas"]["Era"][];
            /**
             * Format: int32
             * @description A play counts (play counts, last played) when it lasted at least
             *     this long, so skipped tracks do not count. 0 counts every start.
             * @default 30
             */
            min_play_s: number;
            /**
             * @description The mood scale, in order (an energy from 1 to 5 by default).
             * @default [
             *       "1",
             *       "2",
             *       "3",
             *       "4",
             *       "5"
             *     ]
             */
            moods: string[];
            /**
             * @description The sound code scale, in order.
             * @default [
             *       "soft",
             *       "medium",
             *       "hard"
             *     ]
             */
            sound_codes: string[];
        };
        /** @description One play. */
        PublicPlay: {
            album?: string | null;
            artist?: string | null;
            /**
             * Format: int64
             * @description Missing while the track plays.
             */
            ended_at_unix_ms?: number | null;
            /** Format: int64 */
            started_at_unix_ms: number;
            title?: string | null;
        };
        /** @description What plays on air. */
        PublicTrack: {
            album?: string | null;
            artist?: string | null;
            /**
             * Format: int64
             * @description From the cue-in to the cue-out.
             */
            duration_ms: number;
            /** Format: int64 */
            position_ms: number;
            /** Format: int64 */
            started_at_unix_ms: number;
            title?: string | null;
        };
        /** @description What a purge of the trash did. */
        PurgeReport: {
            /**
             * Format: int64
             * @description `None` when there was nothing to purge.
             */
            batch_id?: number | null;
            /**
             * Format: int64
             * @description Files removed from the trash (tracks' files and the files that
             *     went with them).
             */
            files: number;
            /**
             * Format: int64
             * @description Tracks removed for good.
             */
            tracks: number;
        };
        QueueEntry: {
            /**
             * Format: int64
             * @description Assigned by the engine. Never reused: a restarted engine resumes its
             *     saved queue with the same IDs and goes on from there.
             */
            entry_id: number;
            track: components["schemas"]["QueuedTrack"];
        };
        /**
         * @description The queue manager. Until rotation and clockwheels (M6), it keeps the
         *     engine's queue filled with the least recently played tracks.
         */
        QueueSettings: {
            /**
             * @description Add tracks when the queue runs low. Off, the station plays silence
             *     once the queue is empty.
             * @default true
             */
            autofill: boolean;
            /**
             * Format: int32
             * @description Tracks kept waiting (cued and queued), 1 to 50.
             * @default 3
             */
            min_queued: number;
        };
        /** @description A track as the engine holds it. */
        QueuedTrack: {
            album?: string | null;
            artist?: string | null;
            /**
             * Format: int32
             * @description Cue points the engine plays between, overrides applied.
             */
            cue_in_ms: number;
            /** Format: int32 */
            cue_out_ms: number;
            /** Format: int32 */
            duration_ms: number;
            /** @description Sends no metadata (a jingle): the stream keeps the last track's. */
            no_metadata: boolean;
            /** @description Relative to the library root. */
            path: string;
            source: components["schemas"]["PlaySource"];
            title?: string | null;
            /**
             * Format: int64
             * @description Library track, when the track comes from the library.
             */
            track_id?: number | null;
        };
        /** @description A gain ramp, in ms on its track's own timeline. */
        RampView: {
            curve: components["schemas"]["FadeCurve"];
            /** Format: int32 */
            length_ms: number;
            /**
             * Format: float
             * @description Level at the quiet end of the ramp, 0 to 1.
             */
            level: number;
            /** Format: int32 */
            start_ms: number;
        };
        /** @description What a MusicBrainz relationships lookup found. */
        RelationsReport: {
            /**
             * Format: int64
             * @description Artists with a MusicBrainz ID looked up.
             */
            artists: number;
            /**
             * Format: int64
             * @description Lookups that failed (tried again next time).
             */
            failed: number;
            /**
             * Format: int64
             * @description New suggestions, between two artists of the station.
             */
            suggestions: number;
        };
        ReleaseChoice: {
            recording_id: string;
            release_id: string;
        };
        RestrictionInput: {
            enabled?: boolean;
            note?: string | null;
            rule: components["schemas"]["Rule"];
            target: components["schemas"]["Target"];
        };
        RestrictionView: {
            enabled: boolean;
            /** Format: int64 */
            id: number;
            note?: string | null;
            rule: components["schemas"]["Rule"];
            target: components["schemas"]["Target"];
            /** @description The track (`artist – title`), artist or category, for people. */
            target_name?: string | null;
        };
        /** @description A release a track may be on, from AcoustID. */
        ReviewCandidate: {
            country?: string | null;
            /** @description `YYYY-MM-DD`, `YYYY-MM` or `YYYY`. */
            date?: string | null;
            /** Format: int32 */
            disc?: number | null;
            recording_id: string;
            release_group_id?: string | null;
            release_group_title?: string | null;
            release_id: string;
            release_title?: string | null;
            /** Format: double */
            score: number;
            /** Format: int32 */
            track?: number | null;
        };
        ReviewItem: {
            /** @description Best first. */
            candidates: components["schemas"]["ReviewCandidate"][];
            /** @description Why the identification was not applied. */
            reason?: string | null;
            /** Format: double */
            score?: number | null;
            track: components["schemas"]["TrackSummary"];
        };
        ReviewPage: {
            items: components["schemas"]["ReviewItem"][];
            /** Format: int64 */
            total: number;
        };
        /**
         * @description What a user may do. Each role can do everything the previous one can.
         * @enum {string}
         */
        Role: "viewer" | "operator" | "admin";
        /** @description A track rotation queued, and why. */
        RotationPick: {
            artist?: string | null;
            explanation: components["schemas"]["Explanation"];
            /** Format: int64 */
            id: number;
            /** Format: int64 */
            planned_at_unix_ms: number;
            /**
             * Format: int64
             * @description When it started; missing until then, or if it never played.
             */
            played_at_unix_ms?: number | null;
            title?: string | null;
            /**
             * Format: int64
             * @description Missing once the track is deleted.
             */
            track_id?: number | null;
        };
        /** @description Station-wide rotation settings, stored under the `rotation` settings name. */
        RotationSettings: {
            /** @default null */
            fallback_category_id: components["schemas"]["u64"] | null;
            /**
             * @default {
             *       "album_min": 60,
             *       "artist_min": 120,
             *       "title_min": 240,
             *       "track_min": 240
             *     }
             */
            separation: components["schemas"]["Windows"];
            /**
             * @default {
             *       "play_factor": 1,
             *       "request_factor": 1
             *     }
             */
            weights: components["schemas"]["WeightSettings"];
        };
        Rule: {
            /** @description ISO 8601: 1 = Monday … 7 = Sunday. */
            days: number[];
            /**
             * Format: int32
             * @description 1 to 24, excluded. At or before `start_hour`, the hours run past
             *     midnight and belong to the day they start on: Friday 22 to 2
             *     covers Saturday 01:30.
             */
            end_hour: number;
            /** @enum {string} */
            kind: "daypart";
            mode: components["schemas"]["Mode"];
            /**
             * Format: int32
             * @description 0 to 23.
             */
            start_hour: number;
        } | {
            /** Format: date */
            end: string;
            /**
             * @description Every year on these days and months (the years are ignored);
             *     the range may run over the new year.
             */
            every_year: boolean;
            /** @enum {string} */
            kind: "date_range";
            mode: components["schemas"]["Mode"];
            /** Format: date */
            start: string;
        } | {
            category_id: components["schemas"]["u64"];
            /** @enum {string} */
            kind: "not_after";
        };
        /** @description Outcome of one scan, logged and served by `/api/v1/library/status`. */
        ScanReport: {
            /** Format: int64 */
            added: number;
            /**
             * Format: int64
             * @description Files modified too recently (probably still being copied).
             */
            deferred: number;
            /** Format: int64 */
            duration_ms: number;
            /**
             * Format: int64
             * @description Analysis jobs queued.
             */
            enqueued: number;
            /**
             * Format: int64
             * @description Files that could not be read.
             */
            failed: number;
            /**
             * Format: int64
             * @description Audio files found on disk.
             */
            files: number;
            /** Format: int64 */
            finished_at_unix: number;
            /**
             * Format: int64
             * @description Tracks newly marked missing.
             */
            missing: number;
            /**
             * Format: int64
             * @description Files found under a new path (same content as a missing track).
             */
            moved: number;
            /**
             * Format: int64
             * @description Unchanged files read again for details an older scanner did not
             *     read (bitrate, genre, …), without being analyzed again.
             */
            refreshed: number;
            /**
             * Format: int64
             * @description Missing tracks whose file is back.
             */
            restored: number;
            /** Format: int64 */
            unchanged: number;
            /** Format: int64 */
            updated: number;
        };
        /** @description The clock running at an hour of the week. */
        ScheduledHour: {
            /** Format: int64 */
            clock_id: number;
            /**
             * Format: int32
             * @description ISO 8601: 1 = Monday … 7 = Sunday.
             */
            day_of_week: number;
            /**
             * Format: int32
             * @description 0 to 23, station time.
             */
            hour: number;
        };
        /** @description What an action on a selection did. */
        SelectionResult: {
            /**
             * Format: int64
             * @description Changes made (a label added to a track counts once).
             */
            changed: number;
            /**
             * Format: int64
             * @description Tracks selected.
             */
            tracks: number;
        };
        /**
         * @description Separation windows overridden by a category or a clock slot; missing
         *     ones are inherited.
         */
        SeparationOverrides: {
            /** Format: int32 */
            album_min?: number | null;
            /** Format: int32 */
            artist_min?: number | null;
            /** Format: int32 */
            title_min?: number | null;
            /** Format: int32 */
            track_min?: number | null;
        };
        /**
         * @description A separation rule.
         * @enum {string}
         */
        SeparationRule: "track" | "title" | "artist" | "album";
        /**
         * @description The engine's silence watchdog: after this long of silence on the master
         *     bus (below −50 dBFS), it skips to the next track, then to the fallback
         *     directory if the silence goes on.
         */
        SilenceWatchdogSettings: {
            /** @default true */
            enabled: boolean;
            /**
             * Format: int32
             * @description Longer than the analysis' longest silence inside a track (15 s), so
             *     it never cuts a track the analysis kept whole.
             * @default 20000
             */
            timeout_ms: number;
        };
        SimulatedPick: {
            explanation: components["schemas"]["Explanation"];
            /** Format: int32 */
            length_ms: number;
            /** Format: int64 */
            start_unix_ms: number;
            track_id: components["schemas"]["u64"];
        };
        Simulation: {
            /** Format: int64 */
            end_unix_ms: number;
            /**
             * Format: int32
             * @description Minutes nothing could be picked (restrictions forbade every track):
             *     the engine's fallback directory would play.
             */
            gap_minutes: number;
            picks: components["schemas"]["SimulatedPick"][];
            /** Format: int64 */
            start_unix_ms: number;
            /**
             * @description Picks per relaxation step, 0 to 10: every pick past step 0 is a
             *     logged relaxation.
             */
            steps: number[];
            violations: components["schemas"]["Violation"][];
        };
        SimulationReport: {
            /** Format: int64 */
            seed: number;
            simulation: components["schemas"]["Simulation"];
            /** @description The tracks picked. */
            tracks: components["schemas"]["TrackLabel"][];
        };
        SimulationRequest: {
            /**
             * Format: int64
             * @description Runs this clock at every hour instead of the schedule.
             */
            clock_id?: number | null;
            /**
             * Format: int32
             * @description 1 to 168 (a week, the default).
             */
            hours?: number | null;
            /**
             * Format: int64
             * @description To replay a simulation; random by default.
             */
            seed?: number | null;
            /**
             * Format: int64
             * @description Now by default, after the plays so far.
             */
            start_unix_ms?: number | null;
        };
        /**
         * @description Why a slot was passed over.
         * @enum {string}
         */
        SkipReason: "nothing_allowed" | "same_type_in_a_row";
        SkippedFile: {
            path: string;
            reason: string;
        };
        SkippedSlot: {
            category_id: components["schemas"]["u64"];
            /** @description Missing in picks logged before reasons were (nothing allowed). */
            reason?: components["schemas"]["SkipReason"];
            /** Format: int32 */
            slot: number;
        };
        /** @description A slot as it will sound. */
        SlotPreview: {
            /**
             * Format: int32
             * @description Their average playable length; missing for an empty category.
             */
            average_ms?: number | null;
            /**
             * @description Expected to start after the hour ends: it will not play (never for
             *     a looping clock).
             */
            beyond_hour: boolean;
            /**
             * Format: double
             * @description Times an hour of this clock calls the category.
             */
            calls_per_hour: number;
            /** Format: int64 */
            category_id: number;
            category_name?: string | null;
            color?: string | null;
            /**
             * Format: int64
             * @description When the slot is expected to start, from the top of the hour.
             */
            expected_start_ms: number;
            /**
             * Format: int32
             * @description Tracks the category needs so its tracks are not repeated within
             *     the track window without relaxation.
             */
            needed_tracks: number;
            /** @description The category has fewer tracks than it needs: expect relaxations. */
            short: boolean;
            /**
             * Format: int32
             * @description Tracks of the category rotation may pick (playable).
             */
            tracks: number;
            /** @description Windows the slot applies (rotation settings, category, slot). */
            windows: components["schemas"]["Windows"];
        };
        /**
         * @description What a file of the buffer zone is imported as, when not its
         *     identification (as stored in `staged_files.choice`).
         */
        StagedChoice: {
            /** Format: int32 */
            disc?: number | null;
            /** @enum {string} */
            kind: "release";
            recording_id: string;
            release_id: string;
            /** Format: int32 */
            track?: number | null;
        } | {
            /** @enum {string} */
            kind: "own";
        };
        StagedFile: {
            /** @description Releases it may be on, best first. */
            candidates: components["schemas"]["ReviewCandidate"][];
            /** @description Categories the track is put in when imported. */
            categories: components["schemas"]["TrackCategory"][];
            choice?: components["schemas"]["StagedChoice"] | null;
            /** Format: int64 */
            created_at_unix_ms: number;
            /** Format: int64 */
            duplicate_of_file_id?: number | null;
            /** Format: int64 */
            duplicate_of_track_id?: number | null;
            /** Format: int32 */
            duration_ms?: number | null;
            error?: string | null;
            /** @description Why it should be looked at before it is imported. */
            flags: components["schemas"]["StagedFlag"][];
            /** Format: int64 */
            group_id: number;
            /** Format: int64 */
            id: number;
            identification?: components["schemas"]["StagedIdentificationView"] | null;
            /** @description Labels the track gets when imported. */
            labels: components["schemas"]["LabelRef"][];
            mood?: string | null;
            /** @description Its path in the upload, or in the incoming folder. */
            name: string;
            own_tags?: components["schemas"]["StagedTags"] | null;
            /** @description Where the layout would file it in the library today. */
            planned_path?: string | null;
            /**
             * Format: int64
             * @description Bytes received so far, while it is uploaded.
             */
            received?: number | null;
            /** @description The release it is imported as (its cover is on the Cover Art Archive). */
            release_id?: string | null;
            /** Format: int64 */
            size: number;
            sound_code?: string | null;
            status: components["schemas"]["StagedStatus"];
            /** @description Fields edited by hand, on top of the tags it gets otherwise. */
            tag_edits: {
                [key: string]: string | null;
            };
            tags?: components["schemas"]["StagedTags"] | null;
            /**
             * Format: int64
             * @description The track it became.
             */
            track_id?: number | null;
            uploaded_by?: string | null;
        };
        /**
         * @description Why a file of the buffer zone should be looked at before it is imported.
         * @enum {string}
         */
        StagedFlag: "duplicate_in_library" | "duplicate_in_buffer" | "same_recording_in_library" | "undecodable" | "too_short" | "unsupported_format";
        StagedGroup: {
            /** Format: int64 */
            created_at_unix_ms: number;
            created_by?: string | null;
            files: components["schemas"]["StagedFile"][];
            /** Format: int64 */
            id: number;
            name: string;
            source: components["schemas"]["GroupSource"];
        };
        /**
         * @description The outcome of identifying a file of the buffer zone.
         * @enum {string}
         */
        StagedIdentificationStatus: "identified" | "needs_review" | "off";
        StagedIdentificationView: {
            /** @description Why it needs review. */
            reason?: string | null;
            recording_id?: string | null;
            release_id?: string | null;
            /** Format: double */
            score?: number | null;
            status: components["schemas"]["StagedIdentificationStatus"];
        };
        /** @enum {string} */
        StagedStatus: "receiving" | "identifying" | "ready" | "importing" | "imported" | "discarded" | "failed";
        StagedTagEdit: {
            file_ids: number[];
            /** @description Fields given back the value they had before they were edited. */
            reset?: components["schemas"]["TagField"][];
            /**
             * @description Fields set to the same value on every file; `null` removes the
             *     field. Fields left out keep each file's value.
             */
            set?: {
                [key: string]: string | null;
            };
        };
        /** @description The tags of a file, as read or as they would be written. */
        StagedTags: {
            album?: string | null;
            album_artist?: string | null;
            artist?: string | null;
            compilation: boolean;
            date?: string | null;
            /** Format: int32 */
            disc?: number | null;
            /** Format: int32 */
            disc_total?: number | null;
            title?: string | null;
            /** Format: int32 */
            track?: number | null;
            /** Format: int32 */
            track_total?: number | null;
        };
        /** @description The station itself. */
        StationSettings: {
            /**
             * @description IANA time zone the clocks and restrictions follow, e.g.
             *     `Europe/Paris`. By default the server's (`TZ`), or UTC.
             */
            timezone: string;
        };
        /** @description Listener statistics. See `docs/statistics.md#settings`. */
        StatisticsSettings: {
            /**
             * Format: int32
             * @description Shortest session that counts as a listener, in seconds (0 counts
             *     every one), at most 3600.
             * @default 60
             */
            min_session_s: number;
            /**
             * Format: int32
             * @description How long sessions and minute samples are kept, 7 to 3650 days.
             * @default 90
             */
            retention_days: number;
        };
        Status: {
            /** @description The server clock's synchronization (the host's: containers share it). */
            clock: components["schemas"]["ClockStatus"];
            /** @description `ok` or `unavailable`. */
            database: string;
            /** @description `ok` or `unavailable`. */
            engine: string;
            /**
             * Format: int64
             * @description The server's time, to compare with the browser's.
             */
            server_time_unix_ms: number;
            service: string;
            version: string;
        };
        SuggestionReview: {
            /** @description `true` marks the artists similar; `false` dismisses the suggestion. */
            accept: boolean;
        };
        /** @enum {string} */
        SyncState: "unknown" | "synced" | "pending" | "failed";
        /** @description Whether the engine runs with the configuration stored in the database. */
        SyncStatus: {
            /** Format: int64 */
            checked_at_unix_ms?: number | null;
            error?: string | null;
            state: components["schemas"]["SyncState"];
        };
        TagEdit: {
            /**
             * @description Programming details written from each track's own values (ISRC,
             *     record label, BPM, mood); tracks without the value keep the file's.
             */
            from_details?: components["schemas"]["TagField"][];
            selection: components["schemas"]["TrackSelection"];
            /**
             * @description Fields set to the same value on every track; `null` removes the
             *     field. Fields left out keep each track's value.
             */
            set?: {
                [key: string]: string | null;
            };
        };
        /**
         * @description A tag Heron edits in the files, besides those identification writes.
         * @enum {string}
         */
        TagField: "title" | "artist" | "album" | "album_artist" | "year" | "genre" | "track" | "disc" | "isrc" | "record_label" | "bpm" | "mood";
        TagOverrides: {
            selection: components["schemas"]["TrackSelection"];
            /**
             * @description Fields set to the same value on every track; `null` removes the
             *     field. Fields left out keep each track's value. Title, artist,
             *     album, album artist, year, genre, track and disc only.
             */
            set: {
                [key: string]: string | null;
            };
        };
        TagOverridesClear: {
            /** @description The fields to give back to the files; every field when empty. */
            fields?: components["schemas"]["TagField"][];
            selection: components["schemas"]["TrackSelection"];
        };
        Target: {
            id: components["schemas"]["u64"];
            /** @enum {string} */
            kind: "track";
        } | {
            id: components["schemas"]["u64"];
            /** @enum {string} */
            kind: "artist";
        } | {
            id: components["schemas"]["u64"];
            /** @enum {string} */
            kind: "category";
        };
        /** @description A task, as stored in `library_tasks.params`. The tag is also its `kind`. */
        TaskRequest: {
            /** @enum {string} */
            kind: "organize_plan";
        } | {
            /** @enum {string} */
            kind: "organize_apply";
            /** Format: int64 */
            plan_task_id: number;
        } | {
            /** Format: int64 */
            batch_id: number;
            /** @enum {string} */
            kind: "undo";
        } | {
            /** Format: int32 */
            disc?: number | null;
            /** @enum {string} */
            kind: "review_apply";
            recording_id: string;
            release_id: string;
            /** Format: int32 */
            track?: number | null;
            /** Format: int64 */
            track_id: number;
        } | {
            /** @enum {string} */
            kind: "artist_relations";
        } | {
            from_details?: components["schemas"]["TagField"][];
            /** @enum {string} */
            kind: "edit_tags";
            set?: {
                [key: string]: string | null;
            };
            track_ids: number[];
        } | {
            all?: boolean;
            /** @enum {string} */
            kind: "musicbrainz_details";
        } | {
            file_ids: number[];
            /** @enum {string} */
            kind: "staging_import";
        } | {
            /** @enum {string} */
            kind: "delete";
            track_ids: number[];
        } | {
            /** @enum {string} */
            kind: "empty_trash";
        };
        /** @enum {string} */
        TaskStatus: "pending" | "running" | "done" | "failed";
        /**
         * @description The web UI's theme.
         * @enum {string}
         */
        Theme: "system" | "light" | "dark";
        TrackAnalysis: {
            /** Format: int64 */
            analyzed_at_unix_ms: number;
            /**
             * Format: double
             * @description The measured tempo.
             */
            bpm?: number | null;
            /** Format: int32 */
            cue_in_ms: number;
            /** Format: int32 */
            cue_out_ms: number;
            /**
             * Format: int32
             * @description The crossfade point: where the next track starts.
             */
            fade_out_start_ms?: number | null;
            has_internal_silence: boolean;
            /** Format: double */
            integrated_lufs?: number | null;
            /** Format: double */
            true_peak_db?: number | null;
        };
        TrackCategory: {
            /** Format: int64 */
            category_id: number;
            color?: string | null;
            name: string;
            /** Format: double */
            weight: number;
        };
        TrackDetail: components["schemas"]["TrackSummary"] & {
            analysis?: components["schemas"]["TrackAnalysis"] | null;
            /**
             * Format: double
             * @description The tempo set by hand, replacing the measured one.
             */
            bpm_by_hand?: number | null;
            categories: components["schemas"]["TrackCategory"][];
            /**
             * Format: int64
             * @description In the trash since then (purged after `trash_days`).
             */
            deleted_at_unix_ms?: number | null;
            file: components["schemas"]["TrackFile"];
            identification?: components["schemas"]["TrackIdentification"] | null;
            markers?: components["schemas"]["TrackMarkers"] | null;
            musicbrainz?: components["schemas"]["TrackMusicBrainz"] | null;
            overrides: components["schemas"]["TrackOverrides"];
            playout: components["schemas"]["EffectivePlayout"];
            /**
             * @description Tags kept in Heron's database (library setting `write_tags` off),
             *     shown instead of the file's: `POST /tracks/tag-overrides/clear`
             *     gives them back to the file.
             */
            tag_overrides: components["schemas"]["TagField"][];
        };
        /** @description A track change reported by the engine. */
        TrackEvent: {
            /** Format: int64 */
            at_unix_ms: number;
            deck: components["schemas"]["DeckName"];
            entry: components["schemas"]["QueueEntry"];
            /** @enum {string} */
            kind: "started";
        } | {
            /** Format: int64 */
            at_unix_ms: number;
            deck: components["schemas"]["DeckName"];
            entry: components["schemas"]["QueueEntry"];
            /** @enum {string} */
            kind: "ended";
            /** Format: int64 */
            played_ms: number;
            reason: components["schemas"]["EndReason"];
        };
        /** @description The file and its tags, as the scanner read them. */
        TrackFile: {
            /** Format: int32 */
            channels?: number | null;
            /** Format: int32 */
            disc_number?: number | null;
            /** Format: int64 */
            missing_since_unix_ms?: number | null;
            musicbrainz_recording_id?: string | null;
            musicbrainz_release_id?: string | null;
            /** Format: int32 */
            sample_rate?: number | null;
            /** Format: int64 */
            size?: number | null;
            /** Format: int32 */
            track_number?: number | null;
            /**
             * Format: int32
             * @description The year tag.
             */
            year_tag?: number | null;
        };
        /** @enum {string} */
        TrackFilter: "playable" | "missing" | "unanalyzed" | "silent" | "disabled" | "needs_review";
        TrackIdentification: {
            /** @description Why the track needs review. */
            reason?: string | null;
            recording_mbid?: string | null;
            release_mbid?: string | null;
            /** Format: double */
            score?: number | null;
            /** @description `identified` or `needs_review`. */
            status: string;
        };
        TrackImport: {
            /** Format: int64 */
            created_at_unix_ms: number;
            created_by?: string | null;
            /** Format: int64 */
            id: number;
            summary: components["schemas"]["ImportSummary"];
            /** Format: int64 */
            undone_at_unix_ms?: number | null;
        };
        TrackLabel: {
            artist?: string | null;
            /** Format: int64 */
            id: number;
            title?: string | null;
        };
        /** @description A track's listeners over its plays of a period. */
        TrackListeners: {
            artist?: string | null;
            /**
             * Format: double
             * @description Departures over the listeners at the start.
             */
            departure_rate?: number | null;
            /**
             * Format: int32
             * @description Sessions of the minimum length that ended during its plays; missing
             *     without listener lists.
             */
            departures?: number | null;
            /**
             * Format: double
             * @description When it ended, on average.
             */
            listeners_end: number;
            /**
             * Format: double
             * @description The station's listeners when it started, on average.
             */
            listeners_start: number;
            /**
             * Format: int32
             * @description Plays with listeners measured when they started.
             */
            plays: number;
            title?: string | null;
            /** Format: int64 */
            track_id: number;
        };
        /** @description Intro, hook and next-track points of a track (`track_markers`). */
        TrackMarkers: {
            /** Format: int32 */
            hook_in_ms?: number | null;
            /** Format: int32 */
            hook_out_ms?: number | null;
            /** Format: int32 */
            intro2_ms?: number | null;
            /** Format: int32 */
            intro_ms?: number | null;
            /** Format: int32 */
            next_hard_ms?: number | null;
            /** Format: int32 */
            next_soft_ms?: number | null;
        };
        /** @description What MusicBrainz says about the track. */
        TrackMusicBrainz: {
            album?: string | null;
            catalog_number?: string | null;
            /** Format: int64 */
            fetched_at_unix_ms: number;
            /** Format: int32 */
            first_release_year?: number | null;
            /** @description Most voted first: suggestions for the genre tag. */
            genres: string[];
            isrc?: string | null;
            record_label?: string | null;
        };
        /**
         * @description Per-track overrides (`track_overrides`). Missing fields use the analysis
         *     and station settings.
         */
        TrackOverrides: {
            /**
             * @default {
             *       "crossfade": null,
             *       "fade_in": null,
             *       "fade_out": null,
             *       "fixed_ms": null,
             *       "mode": null
             *     }
             */
            crossfade: components["schemas"]["CrossfadeOverrides"];
            /**
             * Format: int32
             * @default null
             */
            cue_in_ms: number | null;
            /**
             * Format: int32
             * @default null
             */
            cue_out_ms: number | null;
            /** @default null */
            gap_killer_enabled: boolean | null;
        };
        TrackPage: {
            items: components["schemas"]["TrackSummary"][];
            /**
             * Format: int64
             * @description Matching tracks in all pages.
             */
            total: number;
        };
        TrackPatch: {
            enabled?: boolean | null;
        };
        /**
         * @description Which tracks: the filters of `GET /tracks`, also used to select tracks
         *     for the actions on a selection. Lists of IDs are comma-separated.
         */
        TrackQuery: {
            /**
             * Format: int32
             * @description Tracks added in the last this many days.
             */
            added_days?: number | null;
            /**
             * Format: int64
             * @description Tracks created by this import batch (an import from the buffer zone).
             */
            batch_id?: number | null;
            /**
             * Format: int32
             * @description Tracks whose bitrate is below this many kbps.
             */
            bitrate_below?: number | null;
            /** Format: double */
            bpm_max?: number | null;
            /** Format: double */
            bpm_min?: number | null;
            /** Format: int64 */
            category_id?: number | null;
            /** @description Reverses the order of `sort`. */
            desc?: boolean | null;
            era?: string | null;
            filter?: components["schemas"]["TrackFilter"] | null;
            genre?: string | null;
            /** @description Tracks with every one of these labels (IDs, comma-separated). */
            labels_all?: string | null;
            /** @description Tracks with at least one of these labels. */
            labels_any?: string | null;
            /** @description Tracks with none of these labels. */
            labels_none?: string | null;
            /**
             * Format: int32
             * @description 1 to 500, 50 by default.
             */
            limit?: number | null;
            mood?: string | null;
            /** @description Tracks never played. */
            never_played?: boolean | null;
            /**
             * Format: int32
             * @description Tracks not played for this many days (including those never played).
             */
            not_played_days?: number | null;
            /** Format: int32 */
            offset?: number | null;
            /** @description Words found in the title, artist, album, album artist, genre or path. */
            q?: string | null;
            sort?: components["schemas"]["TrackSort"] | null;
            sound_code?: string | null;
            /** Format: int32 */
            year_max?: number | null;
            /** Format: int32 */
            year_min?: number | null;
        };
        /** @description Some tracks: these IDs, or every track matching the filters. */
        TrackSelection: {
            filters?: components["schemas"]["TrackQuery"] | null;
            track_ids?: number[] | null;
        };
        /** @enum {string} */
        TrackSort: "artist" | "title" | "album" | "added" | "last_played" | "play_count" | "bitrate" | "duration" | "year" | "bpm" | "mood" | "sound_code" | "genre";
        TrackSummary: {
            /**
             * Format: int64
             * @description When the scanner first found the file.
             */
            added_at_unix_ms: number;
            album?: string | null;
            album_artist?: string | null;
            analyzed: boolean;
            artist?: string | null;
            /** Format: int32 */
            bitrate_kbps?: number | null;
            /** @description `cbr`, `vbr` or `lossless`; unknown for some formats (AAC). */
            bitrate_mode?: string | null;
            /**
             * Format: double
             * @description Set by hand, else measured.
             */
            bpm?: number | null;
            /** @description `MP3`, `AAC`, `FLAC`, … */
            codec?: string | null;
            /** Format: int32 */
            duration_ms?: number | null;
            /** @description Disabled tracks are never picked by autofill or rotation. */
            enabled: boolean;
            /** @description From the year, on the station's eras (decades by default). */
            era?: string | null;
            genre?: string | null;
            /** Format: int64 */
            id: number;
            labels: components["schemas"]["LabelRef"][];
            /**
             * Format: int64
             * @description Plays that count (FX player aside, at least the programming
             *     settings' `min_play_s`).
             */
            last_played_at_unix_ms?: number | null;
            missing: boolean;
            mood?: string | null;
            /** @description Relative to the library root. */
            path: string;
            /** Format: int64 */
            play_count: number;
            silent: boolean;
            sound_code?: string | null;
            title?: string | null;
            /**
             * Format: int32
             * @description Of first release: from MusicBrainz, else the year tag.
             */
            year?: number | null;
        };
        TransitionPreview: {
            fade_in?: components["schemas"]["RampView"] | null;
            fade_out?: components["schemas"]["RampView"] | null;
            incoming: components["schemas"]["EffectivePlayout"];
            /**
             * Format: int32
             * @description When the incoming track starts, on the outgoing track's timeline.
             */
            next_start_ms: number;
            outgoing: components["schemas"]["EffectivePlayout"];
            /**
             * Format: int32
             * @description How long both tracks play at once (0: back to back).
             */
            overlap_ms: number;
        };
        TransitionRequest: {
            /** Format: int64 */
            from_track_id: number;
            settings?: components["schemas"]["PlayoutSettings"] | null;
            /** Format: int64 */
            to_track_id: number;
        };
        TrashItem: {
            /**
             * Format: int64
             * @description The `delete` batch: restoring undoes it.
             */
            batch_id?: number | null;
            /** Format: int64 */
            deleted_at_unix_ms: number;
            /** @description Where it was in the library, and goes back to when restored. */
            original_path: string;
            /**
             * Format: int64
             * @description When it is purged for good.
             */
            purge_at_unix_ms: number;
            track: components["schemas"]["TrackSummary"];
        };
        TrashPage: {
            items: components["schemas"]["TrashItem"][];
            /** Format: int64 */
            total: number;
            /**
             * Format: int32
             * @description Days deleted tracks are kept.
             */
            trash_days: number;
        };
        User: {
            /** Format: int64 */
            created_at_unix_ms: number;
            enabled: boolean;
            /**
             * @description The web UI uses the whole width of the screen instead of a centred
             *     column.
             */
            full_width: boolean;
            /** Format: int64 */
            id: number;
            /**
             * @description The web UI's language: a language tag (`en`, `fr`…), or `system` to
             *     follow the browser's languages.
             */
            language: string;
            /** Format: int64 */
            last_login_at_unix_ms?: number | null;
            role: components["schemas"]["Role"];
            theme: components["schemas"]["Theme"];
            username: string;
        };
        UserPatch: {
            /** @description Disabled users cannot sign in, and their tokens stop working. */
            enabled?: boolean | null;
            /** @description Signs the user out everywhere. */
            password?: string | null;
            role?: components["schemas"]["Role"] | null;
        };
        Violation: {
            /** Format: int64 */
            earlier_unix_ms: number;
            /** @enum {string} */
            kind: "separation";
            /**
             * Format: int32
             * @description Index in the picks.
             */
            pick: number;
            rule: components["schemas"]["SeparationRule"];
            /** Format: int32 */
            window_min: number;
        } | {
            /** @enum {string} */
            kind: "restriction";
            /** Format: int32 */
            pick: number;
            /** Format: int64 */
            restriction_id: number;
        } | {
            expected?: components["schemas"]["u64"] | null;
            /** @enum {string} */
            kind: "category";
            /** Format: int32 */
            pick: number;
        } | {
            /** @enum {string} */
            kind: "in_a_row";
            /** Format: int32 */
            pick: number;
            type_id: components["schemas"]["u64"];
        };
        Waveform: {
            /**
             * Format: float
             * @description Level of point 0; 255 is full scale (0 dBFS), on a dB scale.
             */
            floor_db: number;
            /**
             * Format: int32
             * @description Length of one point.
             */
            interval_ms: number;
            /** @description One byte per point, base64: the highest sample peak in the interval. */
            peaks: string;
        };
        /**
         * @description What a delivery reports.
         * @enum {string}
         */
        WebhookEvent: "track.started" | "track.ended" | "track.skipped" | "ping";
        WebhookInput: {
            /**
             * @description With `{{placeholders}}`: `event`, `delivery_id`, `at_unix_ms`,
             *     `track_id`, `artist`, `title`, `album`, `duration_ms`, `deck`,
             *     `source`, `played_ms`, `reason`. Missing or empty: the default JSON
             *     body.
             */
            body_template?: string | null;
            /** @description `application/json` by default. */
            content_type?: string | null;
            enabled?: boolean;
            /** @description At least one of `track.started`, `track.ended`, `track.skipped`. */
            events: components["schemas"]["WebhookEvent"][];
            include_fx?: boolean;
            name: string;
            /** @description `http` or `https`. It may be an address of your network. */
            url: string;
        };
        WebhookSecret: {
            /** @description Shown once. */
            secret: string;
        };
        /** @description A webhook, without its secret. */
        WebhookView: {
            /** @description Missing: the default JSON body (`Payload`). */
            body_template?: string | null;
            content_type: string;
            /** Format: int64 */
            created_at_unix_ms: number;
            enabled: boolean;
            events: components["schemas"]["WebhookEvent"][];
            /** Format: int64 */
            id: number;
            /** @description Also send the FX player's tracks (jingles, liners). */
            include_fx: boolean;
            last_delivery?: components["schemas"]["DeliverySummary"] | null;
            name: string;
            url: string;
        };
        /** @description Weights move with plays and requests (multiplied by these factors). */
        WeightSettings: {
            /**
             * Format: double
             * @description Each play multiplies the track's weights by this, 0.5 to 1 (1: weights stay).
             * @default 1
             */
            play_factor: number;
            /**
             * Format: double
             * @description Each request multiplies the track's weights by this, 1 to 2 (1: weights stay).
             * @default 1
             */
            request_factor: number;
        };
        /** @description Separation windows, in minutes; 0 turns a rule off. */
        Windows: {
            /**
             * Format: int32
             * @description Do not play the same album within.
             * @default 60
             */
            album_min: number;
            /**
             * Format: int32
             * @description Do not play the same artist (or a similar one) within.
             * @default 120
             */
            artist_min: number;
            /**
             * Format: int32
             * @description Do not play the same title (any version) within.
             * @default 240
             */
            title_min: number;
            /**
             * Format: int32
             * @description Do not play the same track within.
             * @default 240
             */
            track_min: number;
        };
        /** Format: int64 */
        u64: number;
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    move_name: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MoveName"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Artist"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_suggestions: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArtistSuggestion"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    look_up_relations: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    review_suggestion: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SuggestionReview"];
            };
        };
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_artists: {
        parameters: {
            query?: {
                /** @description Matches any of the artist's names. */
                q?: string;
                /** @description 1 to 200, 50 by default. */
                limit?: number;
                offset?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ArtistPage"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_artist: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Artist"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    patch_artist: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ArtistPatch"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Artist"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    merge_artist: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MergeRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Artist"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    add_similar: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
                other: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    remove_similar: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
                other: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    login: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LoginRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"];
                };
            };
            401: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            /** @description Too many failures for this username */
            429: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    logout: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    me: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    change_password: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PasswordChange"];
            };
        };
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    set_preferences: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["Preferences"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_categories: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Category"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_category: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoryInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Category"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_category: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Category"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_category: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoryInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Category"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_category: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_types: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CategoryType"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_type: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoryTypeInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CategoryType"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_type: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoryTypeInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CategoryType"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_type: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_clocks: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClockView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_clock: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ClockInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClockView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    preview_clock: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PreviewRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClockPreview"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_clock: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClockView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_clock: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ClockInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClockView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_clock: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_profiles: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EncoderProfileView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_profile: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["EncoderProfileInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EncoderProfileView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_profile: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EncoderProfileView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_profile: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["EncoderProfileInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EncoderProfileView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_profile: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    engine_status: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EngineStatus"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_history: {
        parameters: {
            query?: {
                /** @description Plays started before this time (Unix ms), for the next page. */
                before?: number;
                /** @description Plays started at or after this time (Unix ms). */
                after?: number;
                /** @description Only this track. */
                track_id?: number;
                /** @description Leave out the FX player (jingles played over the music). */
                exclude_fx?: boolean;
                /** @description 1 to 500, 50 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Play"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_labels: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Label"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_label: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Label"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_label: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Label"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_label: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    merge_label: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelMerge"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Label"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_batches: {
        parameters: {
            query?: {
                /** @description 1 to 200, 20 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Batch"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    undo_batch: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ImportApplyRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackImport"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    preview: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ImportPreviewRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ImportPreview"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_imports: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackImport"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    undo: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackImport"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    fetch_details: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["DetailsRequest"];
            };
        };
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    plan_organization: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply_organization: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                plan_task_id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_review: {
        parameters: {
            query?: {
                /** @description 1 to 200, 50 by default. */
                limit?: number;
                offset?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReviewPage"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    library_status: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryStatus"];
                };
            };
            "5XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_tasks: {
        parameters: {
            query?: {
                /** @description 1 to 200, 20 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_task: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_trash: {
        parameters: {
            query?: {
                /** @description 1 to 200, 50 by default. */
                limit?: number;
                offset?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrashPage"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    empty_trash: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    restore_batch: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                batch_id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    days: {
        parameters: {
            query: {
                /** @description First day (`YYYY-MM-DD`, station time). */
                from: string;
                /** @description Last day, included. */
                to: string;
                /** @description Only this output; the whole station otherwise. */
                output_id?: number;
                /** @description Rows per hour, day (by default), week or month. */
                group?: "hour" | "day" | "week" | "month";
                format?: "json" | "csv";
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PeriodTotals"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    hours_of_day: {
        parameters: {
            query: {
                /** @description First day (`YYYY-MM-DD`, station time). */
                from: string;
                /** @description Last day, included. */
                to: string;
                /** @description Only this output; the whole station otherwise. */
                output_id?: number;
                format?: "json" | "csv";
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HourOfDay"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    now: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ListenersNow"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    players: {
        parameters: {
            query: {
                /** @description First day (`YYYY-MM-DD`, station time). */
                from: string;
                /** @description Last day, included. */
                to: string;
                /** @description Only this output; the whole station otherwise. */
                output_id?: number;
                /** @description 1 to 100, 20 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PlayerListening"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    samples: {
        parameters: {
            query: {
                /** @description From this time (Unix ms). */
                from_unix_ms: number;
                /** @description Until this time (Unix ms), at most 8 days after `from_unix_ms`. */
                to_unix_ms: number;
                /** @description Only this output; the whole station otherwise. */
                output_id?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["MinuteSample"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    tracks: {
        parameters: {
            query: {
                /** @description First day (`YYYY-MM-DD`, station time). */
                from: string;
                /** @description Last day, included. */
                to: string;
                /** @description Tracks played fewer times are left out, 3 by default. */
                min_plays?: number;
                /** @description 1 to 500, 50 by default. */
                limit?: number;
                format?: "json" | "csv";
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackListeners"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    live: {
        parameters: {
            query?: {
                /** @description Send levels ten times a second (default true). */
                levels?: boolean;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description WebSocket of LiveMessage */
            101: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LiveMessage"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_outputs: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OutputView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_output: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["OutputInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OutputView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_output: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OutputView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_output: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["OutputInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OutputView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_output: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_player: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PlayerState"];
                };
            };
            /** @description Engine unreachable */
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    play_fx: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PlayFxRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PlayFxResponse"];
                };
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    stop_fx: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_levels: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Levels"];
                };
            };
            /** @description Engine unreachable */
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    pause: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    resume: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    skip: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    preview_transition: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TransitionRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TransitionPreview"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    public_history: {
        parameters: {
            query?: {
                /** @description Plays started before this time (Unix ms), for the next page. */
                before?: number;
                /** @description 1 to 50, 20 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PublicPlay"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    now_playing: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["NowPlaying"];
                };
            };
        };
    };
    get_queue: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["QueueEntry"][];
                };
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    enqueue: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["EnqueueRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EnqueueResponse"];
                };
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    clear_queue: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    remove_entry: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                entry_id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    move_entry: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                entry_id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MoveRequest"];
            };
        };
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_restrictions: {
        parameters: {
            query?: {
                /** @description `track`, `artist` or `category`. */
                target?: string;
                target_id?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RestrictionView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_restriction: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RestrictionInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RestrictionView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_restriction: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RestrictionInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RestrictionView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_restriction: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_picks: {
        parameters: {
            query?: {
                /** @description Picks before this one, for the next page. */
                before?: number;
                /** @description Only this track. */
                track_id?: number;
                /** @description 1 to 200, 50 by default. */
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RotationPick"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    simulate: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SimulationRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SimulationReport"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_schedule: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ScheduledHour"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_schedule: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ScheduledHour"][];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ScheduledHour"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_analysis: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AnalysisSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_analysis: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["AnalysisSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AnalysisSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_library: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibrarySettingsView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    patch_library: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LibrarySettingsPatch"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibrarySettingsView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_playout: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PlayoutSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_playout: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PlayoutSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PlayoutSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_programming: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ProgrammingSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_programming: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ProgrammingSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ProgrammingSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_queue_settings: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["QueueSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_queue_settings: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["QueueSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["QueueSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_rotation_settings: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RotationSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_rotation_settings: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RotationSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RotationSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_station: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StationSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_station: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["StationSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StationSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_statistics: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StatisticsSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_statistics: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["StatisticsSettings"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StatisticsSettings"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_buffer: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Buffer"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    discard_files: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["FileSelection"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Discarded"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    prepare_files: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["Preparation"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Prepared"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    edit_staged_tags: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["StagedTagEdit"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Prepared"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_staged_audio: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The file */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description Part of the file */
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    choose: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ChoiceRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedFile"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_group: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewGroup"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedGroup"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    import_files: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["FileSelection"];
            };
        };
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_upload: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewUpload"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedFile"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_upload: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedFile"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    send_chunk: {
        parameters: {
            query: {
                /** @description Where the chunk goes in the file: the bytes received so far. */
                offset: number;
            };
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/octet-stream": number[];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedFile"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    status: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Status"];
                };
            };
        };
    };
    list_tokens: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiToken"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_token: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewToken"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CreatedToken"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    revoke_token: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_tracks: {
        parameters: {
            query?: {
                /** @description Words found in the title, artist, album, album artist, genre or path. */
                q?: string;
                category_id?: number;
                filter?: "playable" | "missing" | "unanalyzed" | "silent" | "disabled" | "needs_review";
                /** @description Tracks with every one of these labels (IDs, comma-separated). */
                labels_all?: string;
                /** @description Tracks with at least one of these labels. */
                labels_any?: string;
                /** @description Tracks with none of these labels. */
                labels_none?: string;
                /** @description Tracks never played. */
                never_played?: boolean;
                /** @description Tracks not played for this many days (including those never played). */
                not_played_days?: number;
                /** @description Tracks added in the last this many days. */
                added_days?: number;
                /** @description Tracks created by this import batch (an import from the buffer zone). */
                batch_id?: number;
                /** @description Tracks whose bitrate is below this many kbps. */
                bitrate_below?: number;
                era?: string;
                mood?: string;
                sound_code?: string;
                genre?: string;
                year_min?: number;
                year_max?: number;
                bpm_min?: number;
                bpm_max?: number;
                sort?: "artist" | "title" | "album" | "added" | "last_played" | "play_count" | "bitrate" | "duration" | "year" | "bpm" | "mood" | "sound_code" | "genre";
                /** @description Reverses the order of `sort`. */
                desc?: boolean;
                /** @description 1 to 500, 50 by default. */
                limit?: number;
                offset?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackPage"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply_categories: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoriesChange"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SelectionResult"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_tracks: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrackSelection"];
            };
        };
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply_labels: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LabelsChange"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SelectionResult"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply_programming: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ProgrammingChange"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SelectionResult"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    override_tags: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TagOverrides"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SelectionResult"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    clear_tag_overrides: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TagOverridesClear"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SelectionResult"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    edit_tags: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TagEdit"];
            };
        };
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_track: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackDetail"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    patch_track: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrackPatch"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackDetail"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_audio: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description The file */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            /** @description Part of the file */
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_categories: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CategoryAssignment"][];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackCategory"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    put_overrides: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrackOverrides"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TrackOverrides"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_overrides: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    apply_release: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ReleaseChoice"];
            };
        };
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LibraryTask"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    keep_tags: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_waveform: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Waveform"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_users: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_user: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["NewUser"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_user: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_user: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UserPatch"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["User"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    retry_delivery: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DeliveryView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_webhooks: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    create_webhook: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["WebhookInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CreatedWebhook"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    get_webhook: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    update_webhook: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["WebhookInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    delete_webhook: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    list_deliveries: {
        parameters: {
            query?: {
                /** @description Deliveries older than this one (its ID), for the next page. */
                before?: number;
                status?: components["schemas"]["DeliveryStatus"];
                /** @description 1 to 200, 50 by default. */
                limit?: number;
            };
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DeliveryView"][];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    rotate_secret: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookSecret"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
    test_webhook: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DeliveryView"];
                };
            };
            "4XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ErrorBody"];
                };
            };
        };
    };
}
