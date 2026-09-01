/**
 * Player screen — Phase 4 stub. Full playback (react-native-video overlay, the
 * `useTVEventHandler` custom controls, trickplay scrubbing) is Phase 5
 * (`@medi/player`, task `50-phase5-playback-packaging.md`).
 *
 * What it does now: resolve the direct-vs-HLS decision from `/api/stream/:file_id`
 * (exercising the api-client's stream path) and display it. This makes the Detail
 * → Play navigation real and verifiable without pulling the Phase 5 player in.
 */

import React, { useEffect, useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';

import { Page, theme, type StreamDecision } from '../deps';
import { useApi } from '../api';

export function PlayerScreen({
  fileId,
  title,
  isActive,
}: {
  fileId: number;
  title: string;
  isActive: boolean;
}): React.JSX.Element {
  const api = useApi();
  const [decision, setDecision] = useState<StreamDecision | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    api
      .stream(fileId, {}, { signal: controller.signal })
      .then((d) => {
        setDecision(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setError(e instanceof Error ? e.message : 'stream decision failed');
      });
    return () => controller.abort();
  }, [api, fileId]);

  return (
    <Page isActive={isActive}>
      <View style={styles.screen}>
        <Text style={styles.title}>{title}</Text>
        {!decision && !error ? (
          <ActivityIndicator color={theme.colors.text} />
        ) : null}
        {error ? <Text style={styles.error}>Playback unavailable: {error}</Text> : null}
        {decision ? (
          <View style={styles.card}>
            <Text style={styles.line}>
              Mode: <Text style={styles.mono}>{decision.mode}</Text>
            </Text>
            <Text style={styles.line}>
              Reason: <Text style={styles.mono}>{decision.reason}</Text>
            </Text>
            <Text style={styles.line}>
              URL: <Text style={styles.mono}>{api.abs(decision.url)}</Text>
            </Text>
            <Text style={styles.note}>
              Full playback ships in Phase 5 (@medi/player). Press the remote's
              menu/back button to return.
            </Text>
          </View>
        ) : null}
      </View>
    </Page>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: '#000',
    alignItems: 'center',
    justifyContent: 'center',
    padding: theme.screenPaddingH,
  },
  title: { color: theme.colors.text, fontSize: 40, fontWeight: '800', marginBottom: 32 },
  card: {
    backgroundColor: theme.colors.surface,
    padding: 32,
    borderRadius: 12,
    maxWidth: 1000,
  },
  line: { color: theme.colors.text, fontSize: 22, marginBottom: 10 },
  mono: { color: theme.colors.accent, fontWeight: '700' },
  note: { color: theme.colors.textMuted, fontSize: 18, marginTop: 20, lineHeight: 24 },
  error: { color: '#ff6b6b', fontSize: 22, textAlign: 'center' },
});
