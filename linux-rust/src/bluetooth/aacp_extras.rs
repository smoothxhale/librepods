//! AACP features beyond the core protocol: the three-band custom EQ (opcode
//! 0x63), head-tracking sensor reports (opcode 0x17), and device-name checks.
//! Packet offsets count from the start of the full packet, header included.

use crate::bluetooth::aacp::opcodes;

/// Custom EQ as reported by the device. Band gains range 0 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomEq {
    pub enabled: bool,
    pub low: u8,
    pub mid: u8,
    pub high: u8,
}

const EQ_STATE_ENABLED: u8 = 0x02;
const EQ_STATE_DISABLED: u8 = 0x01;
pub const EQ_BAND_MAX: u8 = 100;

impl CustomEq {
    /// Parse a full custom-EQ packet: header, opcode, then
    /// `[len_lo, len_hi, 0x01, state, low, mid, high]`.
    pub fn parse(packet: &[u8]) -> Option<CustomEq> {
        let data = packet.get(6..)?;
        if data.len() < 7 {
            return None;
        }
        Some(CustomEq {
            enabled: data[3] == EQ_STATE_ENABLED,
            low: data[4],
            mid: data[5],
            high: data[6],
        })
    }

    /// Packet (opcode onward) that writes this EQ to the device.
    pub fn to_packet(self) -> [u8; 9] {
        let state = if self.enabled {
            EQ_STATE_ENABLED
        } else {
            EQ_STATE_DISABLED
        };
        [
            opcodes::CUSTOM_EQ,
            0x00,
            0x05,
            0x00,
            0x01,
            state,
            self.low,
            self.mid,
            self.high,
        ]
    }
}

/// Starts the head-tracking sensor stream (motion service 0x10). The device
/// answers a request for service 0x0E with a status-3 refusal on AirPods 4.
pub const HEAD_TRACKING_START: [u8; 23] = [
    opcodes::HEADTRACKING, 0x00, 0x00, 0x00, 0x10, 0x00, 0x0F, 0x00, 0x08, 0x73, 0x42, 0x0B,
    0x08, 0x10, 0x10, 0x02, 0x1A, 0x05, 0x01, 0x40, 0x9C, 0x00, 0x00,
];

/// Stops the head-tracking sensor stream (motion service 0x10).
pub const HEAD_TRACKING_STOP: [u8; 23] = [
    opcodes::HEADTRACKING, 0x00, 0x00, 0x00, 0x10, 0x00, 0x0F, 0x00, 0x08, 0x75, 0x42, 0x0B,
    0x08, 0x10, 0x10, 0x02, 0x1A, 0x05, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// Shortest packet that carries a sensor report. The same opcode also carries a
/// longer sensor descriptor at connect, which is only decoded while tracking.
const REPORT_MIN_LEN: usize = 70;
/// Reports begin with a variable-length counter, so fields keep a fixed distance
/// from the end of the packet. Offsets below are for a report of this length and
/// shift with the actual length (80-byte reports sit one byte earlier).
const REFERENCE_LEN: usize = 81;
/// x, y, z of the orientation quaternion, signed Q15.
const QUATERNION_OFFSETS: [usize; 3] = [43, 45, 47];
const HORIZONTAL_ACCEL_OFFSET: usize = 51;
const VERTICAL_ACCEL_OFFSET: usize = 53;
/// Reports averaged to establish the neutral head position.
const CALIBRATION_SAMPLES: usize = 10;
const Q15_SCALE: f32 = 32768.0;

/// Head orientation in degrees relative to the calibrated neutral position:
/// yaw = turning the head (positive to the wearer's right), pitch = nodding
/// (positive up), roll = tilting toward a shoulder (positive to the right).
/// The earbud sensor's frame is rotated against the head (its z axis follows
/// the nod, its y axis the turn); `process` converts it. Mapping confirmed by
/// observation on AirPods 4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub horizontal_accel: f32,
    pub vertical_accel: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Quat {
    w: f32,
    x: f32,
    y: f32,
    z: f32,
}

impl Quat {
    /// Unit quaternion from its vector part, taking w as non-negative.
    fn from_xyz(x: f32, y: f32, z: f32) -> Quat {
        let w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
        Quat { w, x, y, z }
    }

    fn normalized(self) -> Quat {
        let n = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if n == 0.0 {
            return self;
        }
        Quat {
            w: self.w / n,
            x: self.x / n,
            y: self.y / n,
            z: self.z / n,
        }
    }

    fn conjugate(self) -> Quat {
        Quat {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn mul(self, o: Quat) -> Quat {
        Quat {
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
        }
    }

    /// (yaw, pitch, roll) in degrees, Z-Y-X order.
    fn euler_degrees(self) -> (f32, f32, f32) {
        let Quat { w, x, y, z } = self;
        let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
        let pitch = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
        let roll = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
        (yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees())
    }
}

/// Decodes head-tracking reports; the first reports calibrate the neutral pose.
#[derive(Debug, Default)]
pub struct HeadTracker {
    calibration: Vec<Quat>,
    neutral: Option<Quat>,
    pose: Option<HeadPose>,
}

/// Signed 16-bit little-endian field at `offset` within a REFERENCE_LEN report,
/// relocated for the actual packet length.
fn report_i16(packet: &[u8], offset: usize) -> Option<i16> {
    let at = (offset + packet.len()).checked_sub(REFERENCE_LEN)?;
    let bytes = packet.get(at..at + 2)?;
    Some(i16::from_le_bytes([bytes[0], bytes[1]]))
}

impl HeadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Latest pose, or None while still calibrating.
    pub fn pose(&self) -> Option<HeadPose> {
        self.pose
    }

    pub fn calibrating(&self) -> bool {
        self.neutral.is_none()
    }

    pub fn process(&mut self, packet: &[u8]) {
        if packet.len() < REPORT_MIN_LEN {
            return;
        }
        let Some([x, y, z]) = QUATERNION_OFFSETS
            .iter()
            .map(|&o| report_i16(packet, o))
            .collect::<Option<Vec<_>>>()
            .and_then(|v| <[i16; 3]>::try_from(v).ok())
        else {
            return;
        };
        // The sensor sends all-zero reports while it warms up.
        if x == 0 && y == 0 && z == 0 {
            return;
        }
        let q = Quat::from_xyz(x as f32 / Q15_SCALE, y as f32 / Q15_SCALE, z as f32 / Q15_SCALE);

        let Some(neutral) = self.neutral else {
            self.calibration.push(q);
            if self.calibration.len() >= CALIBRATION_SAMPLES {
                let n = self.calibration.len() as f32;
                let sum = self.calibration.iter().fold(
                    Quat { w: 0.0, x: 0.0, y: 0.0, z: 0.0 },
                    |a, s| Quat { w: a.w + s.w, x: a.x + s.x, y: a.y + s.y, z: a.z + s.z },
                );
                self.neutral = Some(
                    Quat { w: sum.w / n, x: sum.x / n, y: sum.y / n, z: sum.z / n }.normalized(),
                );
            }
            return;
        };

        let (sensor_yaw, sensor_pitch, sensor_roll) = neutral.conjugate().mul(q).euler_degrees();
        let accel = |offset| report_i16(packet, offset).map_or(0.0, f32::from);
        self.pose = Some(HeadPose {
            yaw: sensor_pitch,
            pitch: -sensor_yaw,
            roll: sensor_roll,
            horizontal_accel: accel(HORIZONTAL_ACCEL_OFFSET),
            vertical_accel: accel(VERTICAL_ACCEL_OFFSET),
        });
    }
}

/// Longest device name accepted, in bytes.
pub const NAME_MAX_BYTES: usize = 32;

/// Check a new device name: non-empty, at most NAME_MAX_BYTES of UTF-8, and free
/// of control characters.
pub fn validate_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name must not be empty".to_string());
    }
    if name.len() > NAME_MAX_BYTES {
        return Err(format!("name is {} bytes; the limit is {NAME_MAX_BYTES}", name.len()));
    }
    if name.chars().any(char::is_control) {
        return Err("name must not contain control characters".to_string());
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reported_custom_eq() {
        let packet = hex::decode("040004006300050001024b1f4a").unwrap();
        let eq = CustomEq::parse(&packet).unwrap();
        assert_eq!(
            eq,
            CustomEq {
                enabled: true,
                low: 75,
                mid: 31,
                high: 74
            }
        );
    }

    #[test]
    fn custom_eq_round_trips_through_packet() {
        let eq = CustomEq {
            enabled: false,
            low: 10,
            mid: 50,
            high: 90,
        };
        let mut packet = vec![0x04, 0x00, 0x04, 0x00];
        packet.extend_from_slice(&eq.to_packet());
        assert_eq!(CustomEq::parse(&packet), Some(eq));
    }

    #[test]
    fn short_custom_eq_packet_is_rejected() {
        assert_eq!(CustomEq::parse(&[0x04, 0x00, 0x04, 0x00, 0x63, 0x00, 0x05]), None);
    }

    /// Sensor report of `len` bytes carrying quaternion `q` (vector part, Q15).
    fn report(len: usize, q: Quat) -> Vec<u8> {
        let mut packet = vec![0u8; len];
        let shift = len as isize - REFERENCE_LEN as isize;
        for (offset, value) in QUATERNION_OFFSETS.iter().zip([q.x, q.y, q.z]) {
            let at = (*offset as isize + shift) as usize;
            let raw = (value * Q15_SCALE).round() as i16;
            packet[at..at + 2].copy_from_slice(&raw.to_le_bytes());
        }
        packet
    }

    /// Rotation of `degrees` about the unit axis (ax, ay, az).
    fn rotation(degrees: f32, ax: f32, ay: f32, az: f32) -> Quat {
        let half = degrees.to_radians() / 2.0;
        let s = half.sin();
        Quat { w: half.cos(), x: ax * s, y: ay * s, z: az * s }
    }

    fn calibrated(neutral: Quat) -> HeadTracker {
        let mut tracker = HeadTracker::new();
        for _ in 0..CALIBRATION_SAMPLES {
            tracker.process(&report(81, neutral));
            assert!(tracker.pose().is_none());
        }
        assert!(!tracker.calibrating());
        tracker
    }

    #[test]
    fn sensor_z_rotation_is_a_nod() {
        let neutral = rotation(40.0, 1.0, 0.0, 0.0);
        let mut tracker = calibrated(neutral);
        tracker.process(&report(81, neutral.mul(rotation(30.0, 0.0, 0.0, 1.0))));
        let pose = tracker.pose().unwrap();
        assert!((pose.pitch + 30.0).abs() < 0.1, "pitch {}", pose.pitch);
        assert!(pose.yaw.abs() < 0.1 && pose.roll.abs() < 0.1, "{pose:?}");
    }

    #[test]
    fn sensor_y_rotation_is_a_turn() {
        let neutral = rotation(40.0, 1.0, 0.0, 0.0);
        let mut tracker = calibrated(neutral);
        tracker.process(&report(81, neutral.mul(rotation(-20.0, 0.0, 1.0, 0.0))));
        let pose = tracker.pose().unwrap();
        assert!((pose.yaw + 20.0).abs() < 0.1, "yaw {}", pose.yaw);
        assert!(pose.pitch.abs() < 0.1 && pose.roll.abs() < 0.1, "{pose:?}");
    }

    #[test]
    fn sensor_x_rotation_is_a_tilt() {
        let neutral = rotation(25.0, 0.0, 1.0, 0.0);
        let mut tracker = calibrated(neutral);
        tracker.process(&report(81, neutral.mul(rotation(15.0, 1.0, 0.0, 0.0))));
        let pose = tracker.pose().unwrap();
        assert!((pose.roll - 15.0).abs() < 0.1, "roll {}", pose.roll);
        assert!(pose.yaw.abs() < 0.1 && pose.pitch.abs() < 0.1, "{pose:?}");
    }

    #[test]
    fn head_tracker_decodes_both_report_lengths_alike() {
        let neutral = rotation(25.0, 0.0, 1.0, 0.0);
        let turned = neutral.mul(rotation(15.0, 0.0, 0.0, 1.0));
        let mut short = calibrated(neutral);
        let mut long = calibrated(neutral);
        short.process(&report(80, turned));
        long.process(&report(81, turned));
        let (a, b) = (short.pose().unwrap(), long.pose().unwrap());
        assert!((a.yaw - b.yaw).abs() < 0.01 && (a.pitch - b.pitch).abs() < 0.01);
    }

    #[test]
    fn head_tracker_ignores_warm_up_and_short_packets() {
        let mut tracker = HeadTracker::new();
        tracker.process(&[0u8; 20]);
        for _ in 0..CALIBRATION_SAMPLES * 2 {
            tracker.process(&[0u8; 81]);
        }
        assert!(tracker.calibrating());
    }

    #[test]
    fn validates_names() {
        assert_eq!(validate_name("  My Pods "), Ok("My Pods"));
        assert!(validate_name("").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name(&"x".repeat(NAME_MAX_BYTES + 1)).is_err());
    }
}
