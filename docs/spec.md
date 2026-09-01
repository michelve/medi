1. Package and Library Specification

Database & Backend Engine

SQLite: Embedded database engine. Must be configured for 64KB page sizes to align with modern NVMe drive geometry and utilize Write-Ahead Logging (WAL) for heavy concurrent read access.  

Video Processing & Hardware Acceleration

jellyfin-ffmpeg: A highly patched fork of FFmpeg. Version 5.0.1-5 or newer is strictly required to support Dolby Vision (Profile 5 and 8) to SDR hardware tone-mapping.  

dav1d: Highly optimized AV1 software decoder (included within jellyfin-ffmpeg) to fallback on when AV1 hardware decoding is unavailable.  

intel-media-va-driver-non-free & intel-compute-runtime: Required Linux host drivers injected into the Docker container for Intel QSV and OpenCL tone-mapping capabilities.  

mesa-va-drivers: Required Linux host drivers for AMD Advanced Media Framework (AMF) and VA-API acceleration.  

Cross-Platform Frontend (Apple TV & Android TV)

Expo & react-native-tvos: The core framework for building unified TV applications. The react-native-tvos dependency version must strictly match the Expo SDK version being utilized.  

react-tv-space-navigation: Required for deterministic spatial navigation and handling directional pad (D-Pad) focus states across asymmetrical UI grids.  

react-native-video: The core video playback component with hooks into AVPlayer (Apple) and ExoPlayer (Android).  

xstate: A finite state machine library required to reliably manage the 2-second asynchronous gate for the hover-to-play trailer mechanics without causing race conditions.  

2. Organization & Tooling

Monorepo Architecture: Utilize Yarn workspaces to separate the shared React Native UI components from the platform-specific build configurations.  

Continuous Native Generation (CNG): Use Expo's CNG to entirely abstract the native Xcode and Android Studio project files, allowing them to be dynamically generated during the build pipeline.  

Docker: Utilize a multi-stage, Debian-based Docker build process to compile the backend and safely inject the necessary proprietary user-mode hardware GPU drivers.  

Unraid Community Applications (CA) Tooling: Maintain a dedicated GitHub repository hosting the application's XML schema file and a ca_profile.xml to define the developer's metadata for the Unraid storefront.  

3. Development Roadmap

Phase 1: Foundation & Data Layer: Initialize the backend API and configure the NVMe-optimized SQLite database. Program the background ingestion worker to execute asynchronous ffprobe scans to extract Dolby Vision profiles and HDR transfer characteristics.

Phase 2: Hardware Acceleration Pipeline: Integrate the jellyfin-ffmpeg binary. Write the dynamic command generation logic for Intel QSV, NVIDIA NVENC, and AMD AMF. Ensure OpenCL or CUDA pipelines are explicitly triggered for proprietary Dolby Vision color space transformations.

Phase 3: Background Asset Generation: Build the asynchronous worker process to extract 15-second silent clips during off-peak hours. Hardware-transcode these down to 720p H.264 and generate trickplay sprites (BIF files or tiled JPG matrices) for instantaneous UI loading.

Phase 4: Unified TV Client UI: Scaffold the Expo react-native-tvos monorepo. Implement react-tv-space-navigation for the UI grids and build the xstate state machine to strictly govern the hover-to-play trailer delays.

Phase 5: Playback & Packaging: Integrate react-native-video with custom UI overlays managed by raw remote event interceptors (avoiding spatial navigation conflicts on Android TV). Finalize the Docker image and publish the Unraid XML template with pre-configured GPU hardware passthrough tags.