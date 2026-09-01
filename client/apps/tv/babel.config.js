/**
 * Babel config for the Expo (react-native-tvos) app. `babel-preset-expo` handles
 * the TV variant transparently — it reads the same `EXPO_TV`/plugin signal as the
 * config plugin, so no TV-specific Babel wiring is needed here.
 */
module.exports = function (api) {
  api.cache(true);
  return {
    presets: ['babel-preset-expo'],
  };
};
