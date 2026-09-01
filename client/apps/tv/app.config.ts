/**
 * Expo app config with **Continuous Native Generation** (CNG) for both TV
 * targets (task §Requirements). There are NO checked-in `ios/`/`android/`
 * projects: `expo prebuild` regenerates them from this config + the `@react-native-tvos/config-tv`
 * plugin, which flips the build to tvOS + Android TV.
 *
 * The plugin reads the `EXPO_TV` env var (or its `isTV: true` option) to switch
 * react-native into its TV variant. We pin `isTV: true` here so every prebuild is
 * a TV build; `react-native` itself is aliased to `react-native-tvos@0.81.x` in
 * package.json, whose 0.81 line strictly matches Expo SDK 54 (RN 0.81) as the
 * task requires.
 */

import type { ExpoConfig, ConfigContext } from 'expo/config';

export default ({ config }: ConfigContext): ExpoConfig => ({
  ...config,
  name: 'medi',
  slug: 'medi-tv',
  version: '0.1.0',
  orientation: 'landscape',
  scheme: 'medi',
  platforms: ['ios', 'android'],
  // A single JS entry drives both TV platforms.
  userInterfaceStyle: 'dark',

  ios: {
    bundleIdentifier: 'app.medi.tv',
    // Apple TV build. CNG emits a tvOS target from this.
    supportsTablet: false,
  },

  android: {
    package: 'app.medi.tv',
    // Android TV: declared via the leanback feature in the TV plugin.
  },

  plugins: [
    // The react-native-tvos config plugin. `isTV: true` makes prebuild target
    // Apple TV (tvOS) and Android TV (leanback) from this one config.
    [
      '@react-native-tvos/config-tv',
      {
        isTV: true,
        // tvOS deployment + Android TV banner/leanback are handled by the plugin.
        showVerboseWarnings: false,
      },
    ],
    // react-native-video native module (used by HoverPreview now, full player in Phase 5).
    'react-native-video',
  ],

  experiments: {
    // CNG: no native projects committed; typed routes off (custom navigator).
    typedRoutes: false,
  },

  extra: {
    /**
     * Default backend base URL. On a real LAN appliance this is the server's
     * address; overridable at runtime via the `MEDI_API_BASE_URL` env var picked
     * up here at config eval time.
     */
    apiBaseUrl: process.env.MEDI_API_BASE_URL ?? 'http://localhost:8080',
  },
});
