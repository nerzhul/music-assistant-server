//! Core enums used throughout Music Assistant.
//!
//! Values are serialized as lowercase strings to be wire-compatible with the Python
//! `music_assistant_models.enums` definitions (which use `StrEnum`).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! string_enum {
    ($name:ident { $($variant:ident = $value:literal,)+ }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant,)+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::last()
            }
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }

            pub fn all() -> &'static [Self] {
                &[$(Self::$variant,)+]
            }

            #[doc(hidden)]
            pub fn last() -> Self {
                let mut iter = [$(Self::$variant,)+].into_iter();
                iter.next_back().unwrap()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(format!("unknown {}: {s}", stringify!($name))),
                }
            }
        }
    };
}

string_enum!(MediaType {
    Artist = "artist",
    Album = "album",
    Track = "track",
    Playlist = "playlist",
    Radio = "radio",
    Audiobook = "audiobook",
    Podcast = "podcast",
    PodcastEpisode = "podcast_episode",
    Folder = "folder",
    Announcement = "announcement",
    FlowStream = "flow_stream",
    PluginSource = "plugin_source",
    AudioSource = "audio_source",
    SoundEffect = "sound_effect",
    Genre = "genre",
    Unknown = "unknown",
});

string_enum!(PlaybackState {
    Idle = "idle",
    Paused = "paused",
    Playing = "playing",
    Unknown = "unknown",
});

pub type PlayerState = PlaybackState;

string_enum!(PlayerType {
    Player = "player",
    StereoPair = "stereo_pair",
    Group = "group",
    Protocol = "protocol",
    Display = "display",
    Visualizer = "visualizer",
    Light = "light",
    Unknown = "unknown",
});

string_enum!(PlayerFeature {
    Power = "power",
    VolumeSet = "volume_set",
    VolumeMute = "volume_mute",
    Pause = "pause",
    SetMembers = "set_members",
    MultiDeviceDsp = "multi_device_dsp",
    Seek = "seek",
    NextPrevious = "next_previous",
    PlayAnnouncement = "play_announcement",
    Enqueue = "enqueue",
    SelectSoundMode = "select_sound_mode",
    SelectSource = "select_source",
    Options = "options",
    GaplessPlayback = "gapless_playback",
    GaplessDifferentSamplerate = "gapless_different_samplerate",
    PlayMedia = "play_media",
    Unknown = "unknown",
});

string_enum!(ContentType {
    Ogg = "ogg",
    Wav = "wav",
    Aiff = "aiff",
    Mpeg = "mpeg",
    M4A = "m4a",
    Mp4 = "mp4",
    Mp4A = "mp4a",
    M4B = "m4b",
    Dsf = "dsf",
    Flac = "flac",
    Mp3 = "mp3",
    Wma = "wma",
    WmaV2 = "wmav2",
    WmaPro = "wmapro",
    WavPack = "wavpack",
    Tak = "tak",
    Ape = "ape",
    MusePack = "mpc",
    Aac = "aac",
    Alac = "alac",
    Opus = "opus",
    Vorbis = "vorbis",
    Ac3 = "ac3",
    Eac3 = "eac3",
    Dts = "dts",
    TrueHd = "truehd",
    DtsHd = "dtshd",
    DtsX = "dtsx",
    Cook = "cook",
    Ra144 = "ralf",
    Mp2 = "mp2",
    Mp1 = "mp1",
    Dra = "dra",
    Atrac3 = "atrac3",
    PcmS16Le = "s16le",
    PcmS24Le = "s24le",
    PcmS32Le = "s32le",
    PcmF32Le = "f32le",
    PcmF64Le = "f64le",
    PcmS16Be = "s16be",
    PcmS24Be = "s24be",
    PcmS32Be = "s32be",
    PcmBluray = "pcm_bluray",
    PcmDvd = "pcm_dvd",
    AdpcmIma = "adpcm_ima_qt",
    AdpcmMs = "adpcm_ms",
    AdpcmSwf = "adpcm_swf",
    DsdLsbf = "dsd_lsbf",
    DsdMsbf = "dsd_msbf",
    DsdLsbfPlanar = "dsd_lsbf_planar",
    DsdMsbfPlanar = "dsd_msbf_planar",
    Amr = "amr_nb",
    AmrWb = "amr_wb",
    Speex = "speex",
    PcmAlaw = "alaw",
    PcmMulaw = "mulaw",
    G722 = "g722",
    G726 = "g726",
    Pcm = "pcm",
    Nut = "nut",
    Unknown = "?",
});

impl ContentType {
    pub fn is_pcm(&self) -> bool {
        // Matches Python `ContentType.is_pcm()` which checks the *name* of
        // the variant. The Rust name is upper-cased from the macro ident.
        let name = match self {
            Self::PcmS16Le => "PCM_S16LE",
            Self::PcmS24Le => "PCM_S24LE",
            Self::PcmS32Le => "PCM_S32LE",
            Self::PcmF32Le => "PCM_F32LE",
            Self::PcmF64Le => "PCM_F64LE",
            Self::PcmS16Be => "PCM_S16BE",
            Self::PcmS24Be => "PCM_S24BE",
            Self::PcmS32Be => "PCM_S32BE",
            Self::PcmBluray => "PCM_BLURAY",
            Self::PcmDvd => "PCM_DVD",
            Self::PcmAlaw => "PCM_ALAW",
            Self::PcmMulaw => "PCM_MULAW",
            Self::Pcm => "PCM",
            _ => return false,
        };
        name.starts_with("PCM")
    }

    pub fn is_lossless(&self) -> bool {
        self.is_pcm()
            || matches!(
                self,
                Self::Dsf
                    | Self::Flac
                    | Self::Aiff
                    | Self::Wav
                    | Self::Alac
                    | Self::WavPack
                    | Self::Tak
                    | Self::Ape
                    | Self::TrueHd
                    | Self::DsdLsbf
                    | Self::DsdMsbf
                    | Self::DsdLsbfPlanar
                    | Self::DsdMsbfPlanar
                    | Self::Ra144
            )
    }
}

string_enum!(QueueOption {
    Play = "play",
    Replace = "replace",
    Next = "next",
    ReplaceNext = "replace_next",
    Add = "add",
    Unknown = "unknown",
});

string_enum!(RepeatMode {
    Off = "off",
    One = "one",
    All = "all",
    Unknown = "unknown",
});

string_enum!(AlbumType {
    Album = "album",
    Single = "single",
    Live = "live",
    Soundtrack = "soundtrack",
    Compilation = "compilation",
    Ep = "ep",
    Unknown = "unknown",
});

string_enum!(ArtistType {
    Singer = "singer",
    Author = "author",
    Narrator = "narrator",
    Unknown = "unknown",
});

string_enum!(StreamType {
    Http = "http",
    EncryptedHttp = "encrypted_http",
    Hls = "hls",
    Icy = "icy",
    Shoutcast = "shoutcast",
    InBand = "in_band",
    LocalFile = "local_file",
    NamedPipe = "named_pipe",
    OtherFfmpeg = "other_ffmpeg",
    Custom = "custom",
    Unknown = "unknown",
});

string_enum!(VolumeNormalizationMode {
    Disabled = "disabled",
    Dynamic = "dynamic",
    MeasurementOnly = "measurement_only",
    FallbackFixedGain = "fallback_fixed_gain",
    FixedGain = "fixed_gain",
    FallbackDynamic = "fallback_dynamic",
    Unknown = "unknown",
});

string_enum!(ImageType {
    Thumb = "thumb",
    Landscape = "landscape",
    Fanart = "fanart",
    Logo = "logo",
    Clearart = "clearart",
    Banner = "banner",
    Cutout = "cutout",
    Back = "back",
    Discart = "discart",
    Other = "other",
});

string_enum!(LinkType {
    Website = "website",
    Facebook = "facebook",
    Twitter = "twitter",
    Lastfm = "lastfm",
    Youtube = "youtube",
    Instagram = "instagram",
    Snapchat = "snapchat",
    Tiktok = "tiktok",
    Discogs = "discogs",
    Wikipedia = "wikipedia",
    Allmusic = "allmusic",
    Unknown = "unknown",
});

string_enum!(EventType {
    PlayerAdded = "player_added",
    PlayerUpdated = "player_updated",
    PlayerRemoved = "player_removed",
    PlayerConfigUpdated = "player_config_updated",
    PlayerDspConfigUpdated = "player_dsp_config_updated",
    PlayerOptionsUpdated = "player_options_updated",
    DspPresetsUpdated = "dsp_presets_updated",
    QueueAdded = "queue_added",
    QueueUpdated = "queue_updated",
    QueueItemsUpdated = "queue_items_updated",
    QueueTimeUpdated = "queue_time_updated",
    MediaItemPlayed = "media_item_played",
    MediaItemAdded = "media_item_added",
    MediaItemUpdated = "media_item_updated",
    MediaItemDeleted = "media_item_deleted",
    ProvidersUpdated = "providers_updated",
    SyncTasksUpdated = "sync_tasks_updated",
    TasksUpdated = "tasks_updated",
    MusicSyncCompleted = "music_sync_completed",
    AuthSession = "auth_session",
    CoreStateUpdated = "core_state_updated",
    Shutdown = "application_shutdown",
    Unknown = "unknown",
});

string_enum!(ProviderFeature {
    Browse = "browse",
    Search = "search",
    Recommendations = "recommendations",
    LibraryArtists = "library_artists",
    LibraryAlbums = "library_albums",
    LibraryTracks = "library_tracks",
    LibraryPlaylists = "library_playlists",
    LibraryRadios = "library_radios",
    LibraryAudiobooks = "library_audiobooks",
    LibraryPodcasts = "library_podcasts",
    ArtistAlbums = "artist_albums",
    ArtistTracks = "artist_tracks",
    ArtistTopTracks = "artist_toptracks",
    ArtistTopAlbums = "artist_topalbums",
    LibraryArtistsEdit = "library_artists_edit",
    LibraryAlbumsEdit = "library_albums_edit",
    LibraryTracksEdit = "library_tracks_edit",
    LibraryPlaylistsEdit = "library_playlists_edit",
    LibraryRadiosEdit = "library_radios_edit",
    LibraryAudiobooksEdit = "library_audiobooks_edit",
    LibraryPodcastsEdit = "library_podcasts_edit",
    FavoriteArtistsEdit = "favorite_artists_edit",
    FavoriteAlbumsEdit = "favorite_albums_edit",
    FavoriteTracksEdit = "favorite_tracks_edit",
    FavoritePlaylistsEdit = "favorite_playlists_edit",
    FavoriteRadiosEdit = "favorite_radios_edit",
    FavoriteAudiobooksEdit = "favorite_audiobooks_edit",
    FavoritePodcastsEdit = "favorite_podcasts_edit",
    SimilarTracks = "similar_tracks",
    SimilarArtists = "similar_artists",
    PlaylistTracksEdit = "playlist_tracks_edit",
    PlaylistCreate = "playlist_create",
    PlaylistCreateTracks = "playlist_create_tracks",
    PlaylistCreateAudiobooks = "playlist_create_audiobooks",
    PlaylistCreatePodcastEpisodes = "playlist_create_podcast_episodes",
    PlaylistCreateRadios = "playlist_create_radios",
    PlaylistCreateMixed = "playlist_create_mixed",
    SyncPlayers = "sync_players",
    RemovePlayer = "remove_player",
    CreateGroupPlayer = "create_group_player",
    RemoveGroupPlayer = "remove_group_player",
    ArtistMetadata = "artist_metadata",
    AlbumMetadata = "album_metadata",
    TrackMetadata = "track_metadata",
    Lyrics = "lyrics",
    AudioSource = "audio_source",
    AudioOverlay = "audio_overlay",
    AiQuery = "ai_query",
    Tts = "tts",
    Unknown = "unknown",
});

string_enum!(ProviderType {
    Music = "music",
    Player = "player",
    Metadata = "metadata",
    Plugin = "plugin",
    Core = "core",
    AudioAnalysis = "audio_analysis",
    Unknown = "unknown",
});

string_enum!(ProviderStage {
    Alpha = "alpha",
    Beta = "beta",
    Stable = "stable",
    Experimental = "experimental",
    Unmaintained = "unmaintained",
    Deprecated = "deprecated",
});

string_enum!(CoreState {
    Starting = "starting",
    Running = "running",
    Stopping = "stopping",
    Stopped = "stopped",
});

string_enum!(ConfigEntryType {
    Boolean = "boolean",
    String = "string",
    SecureString = "secure_string",
    Integer = "integer",
    Float = "float",
    Label = "label",
    SplittedString = "splitted_string",
    Divider = "divider",
    Action = "action",
    Icon = "icon",
    Alert = "alert",
    Unknown = "unknown",
});

string_enum!(HidePlayerOption {
    Never = "never",
    WhenOff = "when_off",
    WhenGroupActive = "when_group_active",
    WhenSynced = "when_synced",
    WhenUnavailable = "when_unavailable",
    Always = "always",
});

string_enum!(SourceControl {
    Play = "play",
    Pause = "pause",
    Next = "next",
    Previous = "previous",
    Seek = "seek",
    Unknown = "unknown",
});

string_enum!(TaskStatus {
    Idle = "idle",
    Pending = "pending",
    Running = "running",
    Success = "success",
    PartialSuccess = "partial_success",
    Failed = "failed",
    Cancelled = "cancelled",
    Unknown = "unknown",
});

string_enum!(TaskScheduleType {
    Hourly = "hourly",
    Daily = "daily",
    Weekly = "weekly",
    Unknown = "unknown",
});

string_enum!(ExternalID {
    MbArtist = "musicbrainz_artistid",
    MbAlbum = "musicbrainz_albumid",
    MbReleaseGroup = "musicbrainz_releasegroupid",
    MbTrack = "musicbrainz_trackid",
    MbRecording = "musicbrainz_recordingid",
    Isrc = "isrc",
    Barcode = "barcode",
    Acoustid = "acoustid",
    Asin = "asin",
    Discogs = "discogs",
    Tadb = "tadb",
    Unknown = "unknown",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_round_trip() {
        for ct in ContentType::all() {
            let s = ct.as_str();
            let parsed: ContentType = s.parse().unwrap();
            assert_eq!(*ct, parsed);
            let json = serde_json::to_string(ct).unwrap();
            assert_eq!(json, format!("\"{s}\""));
        }
    }

    #[test]
    fn player_feature_serde() {
        let f = PlayerFeature::VolumeSet;
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(s, "\"volume_set\"");
        let back: PlayerFeature = serde_json::from_str(&s).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn content_type_pcm_detection() {
        assert!(ContentType::PcmS16Le.is_pcm());
        assert!(ContentType::PcmF32Le.is_pcm());
        assert!(!ContentType::Flac.is_pcm());
        assert!(ContentType::Flac.is_lossless());
        assert!(!ContentType::Mp3.is_lossless());
    }
}
