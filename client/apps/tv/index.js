/**
 * App entry. `registerRootComponent` calls `AppRegistry.registerComponent` and
 * ensures the environment is set up for both dev and native builds. Works for the
 * tvOS and Android TV targets alike (CNG).
 */
import { registerRootComponent } from 'expo';

import App from './src/App';

registerRootComponent(App);
